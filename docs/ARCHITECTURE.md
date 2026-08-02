# 架构

## 一句话

Agent 的全部状态活在一个原子依赖图里。源状态是 primitive atom，其余一切（prompt 组装、
pending 汇聚、UI 投影）都是 derived，由引擎重算。因此**记录源状态的变更就等于记录了
全部状态**——undo、redo、崩溃恢复、审计回放共用这一份记录。

## 五个关键判断

**1. 状态引擎不是 Send/Sync，这是刻意的。**
`Store` 是 `Rc<RefCell<Inner>>`，listener 是 `Rc<dyn CellListener>`。这是它同步可重入
语义的代价——listener 里能同步再写，传播是 glitch-free 的。换成 `Arc<Mutex>` 会把重入
变成死锁风险，收益是假的。所以：**每个 session 独占一个线程，store 活在里面**，外界只
通过 `mpsc<Command>` 进、`broadcast<Event>` 出。只有 `agent-server` 知道线程和 tokio
的存在，`agent-core` 永远是单线程视角。

session 的边界是**一个 root agent + 它的整棵子树**——所有子 agent 共用这一个 store 和
这一个线程。子 agent 的并发是 IO 并发（LLM 调用、tool 执行在 IO 池上），状态回写一律
串行回 actor 线程。见 [STATE-MODEL.md](STATE-MODEL.md) §「子 agent」。

**2. prompt 的料单是 derived atom，wire 组装在 adapter。**
换一个 skill 只重算 system 段，加一条消息不重跑 skill 注入——料单（`Ingredients`
的各字段）由引擎增量维护。把料单变成 wire JSON 是模型相关的判断，归 adapter 的
`encode`（决策 15、红线 12），那是个纯函数，输入就是这些 atom 的值。
如果把 store 当成带回调的 HashMap 用，这个架构就没有存在意义——直接写 struct 更快。

**3. 持久化不是后加的功能，它和 undo 是同一份代码。**
恢复 = 从快照开始把 command log 的 `next` 往前推，那正是 redo 的循环。写完 undo/redo，
恢复逻辑就已经写完了。详见 [STATE-MODEL.md](STATE-MODEL.md)。

**4. 工具调用是位置透明的。**
`agent-core` 只发 `ToolCall`，不认识「前端 / 后端」。router 看 descriptor 上的
`location` 决定本地执行还是推到 SSE 上等客户端 POST 回来。两条路径对 core 完全同构：
发出去、置 `Pending`、等回写。详见 [TOOLS.md](TOOLS.md)。

**5. 服务端不做鉴权、日志规范、集群。**
这些是企业的边缘层，每家规范不同，做了他们还得改回去。server 只读 identity header
不验证，只遵守 W3C `traceparent` 不集成任何 APM SDK。企业在自己的网关里加。

## 包结构

```
einfach-agent-rust/
  crates/
    agent-store/        原子引擎 + history + snapshot。fork 自 einfach-core
    agent-core/         AgentValue、原子图、loop 编排、registry/port traits
    agent-providers/    LLM 适配（DeepSeek / Kimi / GLM，一家一个文件夹）
    agent-mcp/          MCP adapter，产出 ToolDescriptor
    agent-server/       库 crate：axum + session actor + HTTP 面
    agent-server-bin/   二十行的 main.rs
  apps/
    desktop/            Tauri 壳，内嵌 agent-server
  packages/
    protocol/           从 Rust 生成的 TS 类型
    client/             传输 + 状态绑定
    ui/                 组件
    web/                浏览器应用
  examples/
    java-gateway/       Spring Boot 参考实现，拷走改，不发版
```

### 各包边界

**`agent-store`** —— 只认识 atom、依赖图、command log。不认识 agent、消息、工具。
泛型或具体值类型由 `agent-core` 定，store 只要求它 `Clone + PartialEq + Serialize`。

**`agent-core`** —— 不做任何 IO。理由不是「要编到 wasm」（wasm 目标已砍），而是
**整个 agent loop 必须能在没有网络的情况下跑单元测试**：mock 一个 provider、mock 一个
tool executor，loop 的状态流转、undo、恢复全部可测。IO 一旦渗进来，这些测试就变成集成
测试，然后就没人写了。

**`agent-server`** —— 是**库**，不是二进制。桌面版内嵌它，企业内部服务也内嵌它，
`agent-server-bin` 只是众多宿主之一。只给二进制的话，企业只能在外面套代理。

```rust
AgentServer::new(config).serve(addr).await
```

## 传输

下行 SSE，上行普通 POST。应用层是全双工，不需要 WebSocket。

```
GET  /sessions/:id/events        SSE：token / tool_call / state / undo_blocked
POST /sessions/:id/input         用户消息
POST /sessions/:id/tool_result   { tool_call_id, epoch, result }
POST /sessions/:id/undo          { granularity: "turn" | "batch" }
POST /sessions/:id/redo
POST /sessions/:id/cancel
```

选 SSE 不选 WS 的理由：更容易过企业代理、更好观测、重连有 `Last-Event-ID` 可以补发。
actor 内保留一个有界事件环形缓冲供补发。

**SSE 响应必须由 server 发出这两个 header**：

```
X-Accel-Buffering: no
Cache-Control: no-cache
```

企业环境里 server 和浏览器之间可能套着 nginx / Ingress Controller / 内部 LB，任何一层
默认缓冲都会把流式变成「一次性吐完」。server 一次发对，所有中间层都老实，网关侧零代码。

### 取消传播

客户端断开 SSE → 必须取消在飞的 LLM 请求和 tool call。这不是运维功能，是正确性：
丢了会白烧 token，还会留下幽灵 session 一直往没人听的 channel 里写。

## 部署形态

server 独立跑，`replicas: 1` 起步。企业网关（他们自己的，或拷走 `examples/java-gateway/`）
挡在前面，server 只有 ClusterIP、不开 Ingress。

`bind` 默认 `127.0.0.1`，要监听 `0.0.0.0` 必须显式设 `AGENT_BIND`。当前没有任何鉴权，
默认安全、暴露是显式动作——裸机上误跑不会裸奔。

### 多副本时的粘性路由

多副本会炸在这里：`GET /events` 落到 Pod-1（session actor 在这），
`POST /tool_result` 落到 Pod-3（找不到这个 session）。

解法是**server 侧自路由**：任意 Pod 查 registry 里这个 session 归谁，不是自己就集群内
转发一跳，SSE 流也照样反代过去。网关完全无感知，它只管打 Service。

```rust
trait SessionRegistry {
    fn owner(&self, id: SessionId) -> Option<PodAddr>;
}
```

`LocalRegistry` 永远命中自己（单副本，转发分支是死代码），多副本时换 `RedisRegistry`,
网关和前端零改动。转发逻辑现在就写，registry 抽象现在就留。

## 边缘无关

| 关注点 | server 侧 | 企业侧 |
|---|---|---|
| 鉴权 | 不做。读 identity header，不验证 | 网关验完写 header |
| 日志 | `tracing` → stderr | 他们的采集 |
| 链路 | 只遵守 W3C `traceparent` | SkyWalking / Sleuth / OTel 都能穿透 |
| 配置 | 环境变量 | ConfigMap / Secret |

**无鉴权 ≠ 无身份。** server 仍然要知道「这是谁的 session」用于隔离与审计归属。
做法是信任上游传入的 `X-Agent-Tenant-Id` / `X-Agent-User-Id`，读不到就落 `anonymous`。
于是企业加鉴权时 server 一行不改，`Entry.owner` 字段现在就留着，以后要多租户不用迁 schema。

## Java 网关参考实现

`examples/java-gateway/`，一百行出头。**不发 Maven、不承诺 Spring Boot 2/3 双兼容、
不跟版。** README 第一句是「拷走改，别当依赖」。

内容：一个 `agent.upstream` 配置、一个 SSE 透传的 `@GetMapping`、五个 POST 转发、
一句 `// 你的 filter 加这里`。

**header 做全量转发**（除 hop-by-hop：`Connection` / `Keep-Alive` / `Transfer-Encoding` /
`Upgrade` / `TE` / `Trailer` / `Proxy-*`），不要逐个白名单复制。全量转发意味着企业加了
鉴权 filter 写什么 header 都自动到 server，`traceparent` 也自动过去——为此专门写的代码
是 0 行。透传不是一个功能，是不做过滤的自然结果。

### SSE 代理的四个坑

参考实现踩了这些，企业拷走就是坏的：

1. **不能缓冲** —— 走 `bodyToFlux(DataBuffer)` 原样转，别 `bodyToMono(String)`，
   否则等上游流结束才发出去。
2. **不能压缩** —— 关掉 gzip 或对这条路径发 `Accept-Encoding: identity`，压缩会把事件
   攒到块边界。
3. **超时放开** —— WebClient 的 response timeout 默认会砍断长连接。
4. **断开要传播** —— 前端关掉 SSE，对上游的订阅必须一起取消。WebFlux 的 Flux 取消会
   自动往上传，但别插 `.cache()` / `.share()` 把取消信号挡住。

**必须用 WebFlux。** Spring MVC 的 `SseEmitter` 一个连接占一个 Tomcat 线程，
默认 `max-threads: 200` 意味着两百个并发会话就把整个应用的线程池吃光，连普通接口都开始
排队。企业存量大多是 MVC，这条必须写进他们的集成文档，并给出两个出路：给
`spring.mvc.async` 配独立线程池，或这条链路单独起一个 WebFlux 服务。

## 桌面版

Tauri 内嵌同一个 `agent-server`（绑 loopback 随机端口，无网关无鉴权）。前端代码一套不变，
只是 base URL 不同。桌面独有能力（fs、shell）以 `location: Desktop` 的 tool 注册进去。

不做 wasm 编译目标。代价是浏览器无法离线自跑，换来少一个 crate、少一个编译目标、
`agent-core` 不用维护 native/wasm 两套 provider。

## Provider 适配

模型侧的差异（缓存语义、`tool_choice` 支持、流式分帧、错误码分配）由
`agent-providers` 的 adapter 吸收，**架构不该知道它们**。

**接缝的完整定义在 [ADAPTER.md](ADAPTER.md)**——料单怎么划、`Adjustment` 怎么报、
trait 长什么样、放错地方有什么症状。这里只留判据：

> 判定一段代码放哪：**它是模型相关的判断吗？** 是 → adapter，不是 → core。
> **core 里一条都不许有**（红线 12）：没有 `match provider`，也没有 `if caps.xxx()`。

两条推论：

1. **请求组装归 adapter**（决策 15）。组装的每个决策都是模型相关的——工具晚加放哪、
   skill 注入到哪、thinking 进不进前缀、temperature 能不能改。
2. **事前问能力改成事后报调整**（决策 17）。core 直接说意图（「这轮必须调 `fs/read`」），
   adapter 做不到就降级并在响应里带一条 `Adjustment`。core 一条路径走到底。

**缓存失效是静默的**——不报错，只是变贵，某些 provider 上是两个数量级的贵。
[024](issues/024-cache-guard.md) 分三层拦，切法同样照红线 12：**判断**归 adapter
（这次该命中多少、哪一段漂了，要看匹配语义和块粒度），**比对**归 core
（预测 vs 真实、滚动窗口，纯算术）。

实测数据与各家差异的完整记录在 [probes/PROVIDERS.md](../probes/PROVIDERS.md)，
它是 adapter 的内部依据，**主线设计一个字都不该引用**。

## 协议类型

`packages/protocol` 的 TS 类型由 Rust 侧用 ts-rs / typeshare 生成，**不手维护**。
线上协议存在两份手写副本是企业级项目最常见的腐坏源。生成步骤进 CI，生成物与源不一致
则构建失败。

这也是选单 monorepo 双 workspace 的唯一理由——协议变更能在一个提交里原子完成。
