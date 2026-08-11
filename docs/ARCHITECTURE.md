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
  crates/                 十个（`Cargo.toml` 的 workspace members 是权威）
    agent-store/        原子引擎 + history + snapshot。fork 自 einfach-core
    agent-core/         AgentValue、原子图、loop 编排、registry/port traits。零 IO
    agent-tools/        内置工具最小集（`srv:fs/read` / `fs/list` / `shell/exec`）+ 本地 executor
    agent-providers/    LLM 适配（DeepSeek / Kimi / GLM，一家一个文件夹）
    agent-transport/    阻塞 HTTP（**全仓唯一允许依赖 ureq** 的 crate）+ providers.toml 解析
    agent-mcp/          MCP adapter，产出 `(ToolSpec, Reversibility)`
    agent-runtime/      把 loop 接到真实 IO：工具表、dispatch、runner pump、skill registry
    agent-cli/          CLI 宿主（lib + main —— 集成测试要走库面才拆的 lib）
    agent-server/       库 crate：axum + session actor + HTTP 面
    agent-server-bin/   默认宿主二进制：二十行的 main.rs + 参数/启动协议（`--ready-file`）
  apps/
    desktop/            Tauri 壳，内嵌 agent-server（`src-tauri` 自带独立 Cargo workspace）
  packages/
    protocol/           从 Rust 生成的 TS 类型
    web/                浏览器应用：传输、状态绑定、组件、MCP 客户端都在 `src/` 下
  examples/
    java-gateway/       Spring Boot 参考实现，拷走改，不发版
  probes/api/           独立 workspace，不进主依赖图
```

`agent-runtime` 是 IO 与纯逻辑的接缝层：`agent-core` 吐 `Effect`，它真的执行掉再把结果
翻译回 `Event` 喂回去。**工具表和 dispatch 在这里，不在 core**——core 没有工具表是刻意的
（`Reversibility` 是描述符上的元数据，core 现造一个等于编造）。

前端曾计划拆成 `client`（传输 + 状态绑定）/ `ui`（组件）两个包，**实际没拆**：两者都在
`packages/web/src/` 下。真出现第二个前端宿主再拆——现在拆就是为一个消费者维护三个包。

### 各包边界

**`agent-store`** —— 只认识 atom、依赖图、command log。不认识 agent、消息、工具。
泛型或具体值类型由 `agent-core` 定，store 只要求它 `Clone + PartialEq + Serialize`。

**`agent-core`** —— 不做任何 IO。理由不是「要编到 wasm」（那是决策 26 的顺带结果，不是
这条约束的理由——哪天不编 wasm 了它也不该松），而是
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
POST /sessions                        建/接会话（chatid 幂等三态，INTEGRATION.md §三）
GET  /sessions/{id}                   会话状态
GET  /sessions/{id}/agents            活 agent 树快照（048，一次派生读，不是新状态）
GET  /sessions/{id}/events            SSE 下行
GET  /sessions/{id}/events/poll       拉取式下行（同一个 ring 的第二个投影，M9 决策 25）
POST /sessions/{id}/input             用户消息
POST /sessions/{id}/tool_result       { agent, tool_call_id, result }
POST /sessions/{id}/undo              { granularity: "turn" | "step", force: bool }
POST /sessions/{id}/redo
POST /sessions/{id}/cancel
```

`tool_result` 的 body **没有 epoch，客户端也指定不了**：epoch 由服务端派发时自己记下，
校验在 actor 持有的 `RunnerCtx` 里精确匹配仍在等待的 `(agent, call_id)`。红线 6 照旧成立，
只是落点不在 wire 上——客户端能伪造世代号的话，红线 6 就成了自证。

`granularity: "step"` 不接受 `force: true`（`Session` 只有 turn 粒度有越过屏障那一档），
这个组合在 HTTP 层就 400，不留到 actor 里被静默忽略。

**事件名不在这份文档里维护。** 权威是 `packages/protocol/src/generated/SessionEvent.ts`
（由 Rust 的 `crates/agent-server/src/event/` 用 ts-rs 生成）。理由见 §协议类型：线上协议
存在两份手写副本是最常见的腐坏源，文档里再抄一份就是第三份。

wire 上每一帧的形状是**信封**（034）：

```
id: 42
data: {"agent":"<AgentId>","event":{"type":"text_delta","data":"…"}}
```

`agent` 是这一帧归哪个 agent（整棵树共用一条流），`event` 才是 `SessionEvent`（邻接标签，
`tag = "type", content = "data"`）。**所有帧都走 SSE 默认的 `message` 事件类型**——没有用
`event:` 命名字段分流，客户端一律 `onmessage` + 按 `event.type` 分派。

选 SSE 不选 WS 的理由：更容易过企业代理、更好观测、重连有 `Last-Event-ID` 可以补发。
补发用的有界事件环形缓冲**在 HTTP 层的 per-session hub 里**（`http/hub/ring.rs`，默认 256
帧），不在 session actor 里。**位置决定语义**：ring 是进程内的，进程重启后 `Last-Event-ID`
补不回旧帧——这是形态的诚实边界，不是缺陷。同一个 ring 也是 `events/poll` 的真值源
（SSE / 轮询 / 长轮询 = 一个 ring 的三种投影，见 [INTEGRATION.md](INTEGRATION.md) §四）。

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

机制是订阅引用计数 + 宽限倒计时：归零才起倒数，到点二次确认仍为零才真取消。
**拉取式共用同一套**——每次 poll 全程持有一个订阅（长轮询挂住期间计数非零，不会误取消；
客户端跑路就没有下一次 poll，倒计时到点走同一条取消路）。同一 session 上一个 SSE 观众
加一个拉取网关，走掉一个不会误杀另一个。

## 部署形态

两种形态，同一个二进制/库：

1. **独立跑**，`replicas: 1` 起步。企业网关挡在前面，server 只有 ClusterIP、不开 Ingress。
2. **当宿主的子进程跑**（M9 起的推荐形态）：Java 网关 / Tauri 用 `--port 0` +
   `--ready-file` 拉起它，于是变成**一个部署单元**，起停/重启/健康归宿主管。

`bind` 默认 `127.0.0.1`，要监听 `0.0.0.0` 必须显式设 `AGENT_BIND`。当前没有任何鉴权，
默认安全、暴露是显式动作——裸机上误跑不会裸奔。子进程形态天然只在本机，这条白拿。

**`--ready-file` 是宿主↔Rust 的稳定启动协议，不要解析启动横幅**：宿主为每次启动给一个
**尚不存在**的路径，Rust 在 bind 成功之后原子发布 `{"port","pid","version"}`（用 `hard_link`
不用 `rename`——后者会静默覆盖，前者要求目标不存在，于是上一次启动留下的陈旧文件不可能
被当成这一次的成功）。**发布失败 = 非零退出**，不会出现「进程在跑但父进程永远等不到文件」。

联调/验收时前端可以由 server 同源托管：`AGENT_STATIC_DIR` 指向 `packages/web` 的
`dist/`（`crates/agent-server/examples/serve.rs` 认它），省掉一个 dev server 和跨域配置。

### 多副本时的粘性路由（**设计草案，未实现**）

> **现状**：下面整节一行代码都没有。`PodAddr` / `LocalRegistry` / `RedisRegistry` /
> 跨 Pod 转发全仓零定义；现有的 `crates/agent-server/src/registry/` 是**单副本内存表**
> （`SessionId → SessionHandle`，`open`/`get`/`close` + 崩溃时的 dead 标记），
> 它的模块文档自己写着：那个 `trait SessionRegistry` 是「`RedisRegistry` 落地时才长出来的
> 接缝」。**多副本现在跑不了**，别照这节做容量规划。保留它是因为它是有效的设计意图——
> 真要做多副本时从这里接着想，不用重新推一遍。

多副本会炸在这里：`GET /events` 落到 Pod-1（session actor 在这），
`POST /tool_result` 落到 Pod-3（找不到这个 session）。

设想的解法是**server 侧自路由**：任意 Pod 查 registry 里这个 session 归谁，不是自己就
集群内转发一跳，SSE 流也照样反代过去。网关完全无感知，它只管打 Service。

```rust
// 草案，尚未存在于代码里
trait SessionRegistry {
    fn owner(&self, id: SessionId) -> Option<PodAddr>;
}
```

`LocalRegistry` 永远命中自己（单副本，转发分支是死代码），多副本时换 `RedisRegistry`，
网关和前端零改动——这是这个形状值得留着的地方：**多副本不该让网关和前端改协议**。

原稿写的是「转发逻辑现在就写，registry 抽象现在就留」。**没这么做**，而且回头看是对的：
单副本下转发分支是纯死代码，写了也验不了；M9 的拉取式传输还让这件事更容易（没有长连接
要粘）。代价是「换个 registry 实现就有多副本」不成立——跨 Pod 转发（含 SSE 反代）整块
都还没做。真排期时见 `docs/issues/README.md` 的未排期段。

## 边缘无关

| 关注点 | server 侧 | 企业侧 |
|---|---|---|
| 鉴权 | 不做。读 identity header，不验证 | 网关验完写 header |
| 日志 | `tracing` → stderr | 他们的采集 |
| 链路 | 只遵守 W3C `traceparent` | SkyWalking / Sleuth / OTel 都能穿透 |
| 配置 | 环境变量 | ConfigMap / Secret |

**无鉴权 ≠ 无身份。** server 仍然要知道「这是谁的 session」用于隔离与审计归属。
**打算**信任上游传入的 `X-Agent-Tenant-Id` / `X-Agent-User-Id`，读不到落 `anonymous`。

> **现状（未实现，未排期）**：server 端**从不读这两个 header**，`EntryMeta` 上**没有
> `owner` 字段**（它现在是 `{ turn_id, epoch, label, barrier }`）。原稿写的
> 「`Entry.owner` 字段现在就留着，以后要多租户不用迁 schema」**是一句反向承诺**：
> 字段不存在，而 `EntryMeta` 是 `Serialize` 的落盘结构——真做多租户时给它加字段
> **就是一次快照/日志的 schema 变更**，必须迁。照原话做多租户规划会踩空。

身份这条现在唯一落地的形态是 **chatid**：`POST /sessions` 接受客户端指定的 id，
**归属由网关保证**（猜到别人的 chatid 就能接上别人的会话）。这是**部署契约**，
不是代码能自己保证的事，完整约束见 [INTEGRATION.md](INTEGRATION.md) §三。

多租户真排期时要一起决的两件事：header 落到哪个 atom（还是只进日志元数据），
以及 `EntryMeta` 加 `owner` 的**落盘兼容策略**（老日志没有这个字段，恢复要能读旧格式）。

## Java 网关参考实现

`examples/java-gateway/`。**不发 Maven、不承诺 Spring Boot 2/3 双兼容、不跟版。**
README 第一句是「拷走改，别当依赖」。已用 OpenJDK 21 + Maven 3.9.15 `mvn -q package`
构建验证过，并在 M9 收官时跑完真机全链（真 deepseek 上游 + curl，网关自己拉起 Rust
子进程、67 帧逐帧到达、停 Java 时 Rust 一起干净退出）——见
[058](issues/058-java-gateway-pull.md)。（037 那句「本机无 JDK、只做源码审阅」已作废；
但**那条规矩仍然有效**：将来在没有 JDK 的环境里维护它，如实标注构建验证缺席，不许伪造。）

### 形态：拉取上游，自己产生 SSE（M9 决策 25）

```
浏览器 ──SSE──> Java 网关 ──长轮询(HTTP)──> Rust agent-server
                     └────── 子进程，生命周期归 Java ──────┘
```

**核心判断：SSE 的复杂度只该出现在「产生 SSE」那一跳，不该出现在「代理 SSE」那一跳。**
产生 SSE 是 Spring 的教程级操作；代理 SSE 才是坑窝（见下）。所以 Java↔Rust 这一跳换成
拉取式，浏览器那一跳的 SSE **协议一行不改**（`EventSource` 的自动重连、`Last-Event-ID`、
帧信封逐字节同形）。完整推导在 [INTEGRATION.md](INTEGRATION.md)。

现在的内容：

- `AgentServerProcess` —— `@PostConstruct` 用 `ProcessBuilder` 拉起 Rust 子进程
  （`--port 0 --ready-file <独占临时路径>`），轮询就绪文件拿到实际端口、校验 pid，
  全程**不解析启动横幅**；`@PreDestroy` `destroy()`（SIGTERM）→ `waitFor` →
  超时才 `destroyForcibly()`。Rust 侧 SIGTERM 会**所有会话落盘之后才退**。
- `AgentSseController` —— 一个自己产生 SSE 的 `@GetMapping`，内部循环拉上游
  `GET /sessions/{id}/events/poll`，带 `Last-Event-ID: <游标>` + `X-Poll-Wait-Ms: 25000`。
  **`next` 由服务端算，网关不加一**（ring 只回 `id > Last-Event-ID`，自己 +1 会跳帧）。
- `AgentProxyController` —— catch-all `@RequestMapping("/agent/**")`，转发其余短请求，
  一句 `// 你的 filter 加这里`。
- `ChatSubscribers` —— 本网关每个 chatid 还剩几条浏览器连接，**只用来决定要不要主动发
  `POST /cancel`**。它不是 Rust 订阅计数的副本：那一份还包含直连的客户端和别的网关实例，
  并且仍然是取消的权威（引用计数 → 宽限 → `cancel()`）。

**用长轮询不用短轮询**：拉取式期间网关持有 Rust 侧的 `SubscriberGuard`，短轮询下 guard
在响应发出即释放，轮询间隔必须小于宽限（5s）否则会被判成断开。长轮询全程持有，既没有这个
约束，也把空转请求降到最低。

**header 做全量转发**（除 hop-by-hop：`Connection` / `Keep-Alive` / `Transfer-Encoding` /
`Upgrade` / `TE` / `Trailer` / `Proxy-*`），不要逐个白名单复制。全量转发意味着企业加了
鉴权 filter 写什么 header 都自动到 server，`traceparent` 也自动过去——为此专门写的代码
是 0 行。透传不是一个功能，是不做过滤的自然结果。

**部署契约（代码解决不了的那条）**：chatid 就是会话身份，猜到别人的 chatid 就能接上别人的
会话。网关必须保证归属——chatid 含 uuid，或网关侧做 `user → chatid` 授权校验。

### SSE 代理的四个坑（这条链上**结构性地不存在**了，但拷走前要认得）

这四条只属于**转发**别人的流，不属于**产生**自己的流。拉取式把 Java↔Rust 那一跳的流
删掉了，于是四条一起消失：没有流要缓冲、没有块边界要担心、没有长连接要放开超时、
没有取消信号要往上游传。**留在这里是因为企业很可能在别处仍要代理 SSE**（比如网关前面
再套一层，或直连 `GET /events`）：

1. **不能缓冲** —— 走 `bodyToFlux(DataBuffer)` 原样转，别 `bodyToMono(String)`，
   否则等上游流结束才发出去。
2. **不能压缩** —— 关掉 gzip 或对这条路径发 `Accept-Encoding: identity`，压缩会把事件
   攒到块边界。
3. **超时放开** —— WebClient 的 response timeout 默认会砍断长连接。（拉取式下这条以另
   一种形式回来了：给拉取用的 client 配一个比长轮询上限更短的响应超时，会把**正常空等**
   判成失败。参考实现因此不设 `responseTimeout`。）
4. **断开要传播** —— 前端关掉 SSE，对上游的订阅必须一起取消。WebFlux 的 Flux 取消会
   自动往上传，但别插 `.cache()` / `.share()` 把取消信号挡住。

**「必须用 WebFlux」已经不再由 Rust 侧强制。** 原来的理由是代理 SSE：Spring MVC 的
`SseEmitter` 一个连接占一个 Tomcat 线程，默认 `max-threads: 200` 意味着两百个并发会话就
吃光整个应用的线程池。拉取式之后，**MVC 也能照这个协议实现**——唯一还成立的约束是
别为每条浏览器连接长期占一个请求线程（`spring.mvc.async` 配独立线程池，或这条链路单独
起 WebFlux）。参考实现**仍然**是 WebFlux（`pom.xml`），那是实现选择，不再是硬要求。

## 桌面版

Tauri 内嵌同一个 `agent-server`（绑 loopback 随机端口，无网关无鉴权）。前端代码一套不变，
只是 base URL 不同。这个「内嵌 + loopback 随机端口 + 只换 base URL」的模式后来被 M9 的
Java 网关原样复用（§Java 网关），是它先验证的。

桌面独有能力（fs、shell）的**设计**是以 `location: Desktop` 的 tool 注册进去，走跟 `web:`
完全相同的远端通道（`Location::is_remote()` 两者同真，前缀 `desk:`，`ToolTable` 认这个前缀）。

> **现状：一个 `desk:` 工具都没注册。** 路由那半边是通的（跟 `web:` 共用），但没有任何
> 装配路径注册桌面工具，也没有端到端测试（端到端只有 Web 侧）。`bootstrap.rs` 明写
> 「真需要调时再往 `BootstrapOptions` 加，不提前造」。不是缺陷，是没到需要的时候——
> 但别把「设计好了」当成「能用了」。

**wasm 是第三种宿主形态**（决策 26，2026-08-10，取代原先「不做 wasm 编译目标」）：核心编进
浏览器直接跑，没有 agent-server 进程。三种形态并存，决策 12 的「`agent-server` 是库」不变。

浏览器形态下的裁剪：不编 `agent-mcp`（stdio 不存在；浏览器够得着的 MCP 由前端自己连，
见 [HOST-CAPABILITIES.md](HOST-CAPABILITIES.md) §七），不声明 `agent-tools` 的 `srv:` shell/fs
specs（纯数据，不声明即可），`agent-transport` 换 fetch 实现——`fetch` 的流式响应体加
`AbortController` 正好顶掉 `read_loop.rs` 为绕开 ureq 无中断句柄而写的那一整套。

**唯一的结构性改动**是 `RunnerCtx.fs: ToolExecutor`：它是 concrete struct，`new()` 要
canonicalize 一个真实目录，浏览器里没有。要开一个注入接缝——顺带一提，本文上面说「mock
一个 tool executor」，按当前结构那个接缝**其实还不存在**，随这次一起开。
展开见 [issues/111](issues/111-wasm-target-decision.md)–[114](issues/114-wasm-host.md)。

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

`packages/protocol` 的 TS 类型由 Rust 侧用 **ts-rs** 生成，**不手维护**（原稿写的
「ts-rs / typeshare」里的 typeshare 从未用过，只有 ts-rs）。线上协议存在两份手写副本是
企业级项目最常见的腐坏源。协议改动的本地收工步骤必须打开 `ts` feature 运行协议一致性测试，
生成物与源不一致时测试失败——忘了重新生成 TS 就会在那里红。

这也是选单 monorepo 双 workspace 的唯一理由——协议变更能在一个提交里原子完成。
