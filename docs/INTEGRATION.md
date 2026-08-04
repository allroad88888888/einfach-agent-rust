# 企业集成接缝：会话身份、拉取式传输、进程生命周期

接缝定义文档。管「**企业把这套运行时装进自己的 Java 服务**」这一件事——三个真实约束
逼出来的三条设计。落地里程碑 **M9**，issue 见 055-058。

与既有接缝文档并列：[ADAPTER.md](ADAPTER.md)（模型差异）、[MCP.md](MCP.md)（外部工具）、
[OBSERVABILITY.md](OBSERVABILITY.md)（给人看）、[ORCHESTRATION.md](ORCHESTRATION.md)（给模型看）。
这一份管**给企业宿主看**。

## 一、三个约束（都来自真实提问，不是设想）

1. **Java 网关代理 SSE 有四个坑**（不能缓冲 / 不能压缩 / 超时放开 / 断开要传播），
   而且**强制 WebFlux**——MVC 的 `SseEmitter` 一个连接占一个 Tomcat 线程，默认
   `max-threads: 200` 意味着两百个并发会话吃光整个应用的线程池（ARCHITECTURE §Java 网关）。
   企业存量大多是 MVC。
2. **Java 不控制 Rust 的生死**：现在 server 是独立进程，生命周期归 K8s/systemd，
   Java 只是挡在前面的反向代理。启停不受 Java 控制。
3. **会话身份是业务侧的 `chatid`**，不是 server 生成的随机 id。业务侧拿 chatid 来问：
   这个 chat 有历史吗？有就接上，没有就建一个。

## 二、整体形状

```
浏览器 ──SSE──> Java 网关 ──拉取(HTTP)──> Rust agent-server
                     └────── 子进程，生命周期归 Java ──────┘
```

**核心判断：SSE 的复杂度只该出现在「产生 SSE」那一跳，不该出现在「代理 SSE」那一跳。**

- **产生** SSE（Java → 浏览器）：Spring 的标准做法，教程级，企业已经会。
- **代理** SSE（Rust → Java → 浏览器）：那四个坑全部在这一跳，且强制 WebFlux。

把 Java↔Rust 换成**拉取式**，四个坑一次性消失（没有流要缓冲、没有块边界要担心、没有长
连接要放开超时、没有取消信号要传播），MVC 也能扛。**浏览器那一跳的 SSE 一行不改**——
它本来就适合浏览器（`EventSource` 原生支持、自动重连）。

## 三、会话身份：`chatid` 幂等 getOrCreate

### 现状与改动

`POST /sessions` 现在是 `state.generate_id()`——**服务端生成、客户端不能指定**。改成
接受客户端指定的 id（即 chatid），语义**幂等三态**：

| chatid 的状态 | 行为 | 状态码 |
|---|---|---|
| registry 里活着 | 直接接上（不新建、不清空） | 200 |
| registry 没有、磁盘有 `<dir>/<chatid>.jsonl` | **恢复**（走既有 recover 路） | 200 |
| 都没有 | 新建 | 201 |

**几乎全是白拿**：`SessionId` 本来就是 `Arc<str>`（有 `From<&str>`），`default_sessions_dir`
本来就自动落 `<dir>/<id>.jsonl`，「从磁盘恢复」本来就是 kill-9 重启走的那条路
（`agent_runtime::recover`）。**「查历史」不需要新机制**——磁盘上有那个文件就是有历史。

### 安全点一：路径穿越必须挡（这条不做就是把文件系统交出去）

chatid 由客户端给、又直接拼进文件名，`../../etc/passwd` 这种就是事故。**id 白名单校验**：
只允许 `[A-Za-z0-9_-]`、限长（建议 ≤128），不合规 **400 直接拒**，不做 sanitize
（悄悄改写会让两个不同的 chatid 撞进同一个会话文件——比拒绝更坏）。

校验点在**接受 id 的入口**（`POST /sessions` 与任何按 id 取会话的路径），不是在拼路径的
地方——拼路径的地方可能有好几处，入口只有一处。

### 安全点二：chatid 即身份，归属由网关保证

server 无鉴权是 by design（网关挡前面，ARCHITECTURE §部署形态）。但一旦 chatid 是会话
身份，**猜到别人的 chatid 就能接上别人的会话**。所以：

- **网关必须保证 chatid 的归属**——不能让用户 A 拿到用户 B 的 chatid。
- 推荐 chatid 含不可猜部分（uuid），或网关侧做 `user → chatid` 的授权校验。
- 这跟 ARCHITECTURE 已有的 `X-Agent-Tenant-Id` / `X-Agent-User-Id` 透传是同一套思路：
  **server 知道「这是谁的 session」用于隔离与审计归属，但不做鉴权**。

这条写进文档是因为它是**部署契约**，不是代码能自己保证的事。裸奔的 server + 可猜的
chatid = 越权读别人对话。

## 四、传输：ring 的第二个投影

### 真值源是 ring，不是 SSE

```
SessionHandle(broadcast<Frame>)
      ↓ 一条 drain 任务
  ring::RingState        ← 真值源：单调帧 id + 有界环形缓冲（默认 256 帧）
      ↓ ring::Replay
  每条 SSE 连接一条转发任务（补发 backlog + 续上直播）
```

`RingState::replay(Option<last_event_id>)` **已经写好了**，三个变体正是拉取式需要的全部：

- `Live`——缓冲空，没有可补的
- `Backlog(Vec<BufferedFrame>)`——精确补发（可能为空 = 已追上）
- `Gap { skipped, gap_frame_id, tail }`——缺口（`skipped` 是精确值不是估计）+ **仍保留的
  尾巴**（031 独测分歧 2 的裁决：gap 只代表被冲掉那段，不放弃全部补发）

帧 id 从 1 单调递增，**0 专门留给「客户端从没见过任何帧」**——所以 `since=0` 天然落进
「从头补」而不是被误判成一个真实存在过的帧。

**所以加拉取式不是造新机制，是给同一个 ring 加第二个消费者。** 这跟本仓一贯做法同构：
undo/redo/恢复/审计是一套机制的四个投影，活树是 atom 的派生读，这里是 **SSE / 轮询 /
长轮询 = 同一个 ring 的三种投影**。

### 端点形状：游标走 header，不走 query

```
GET /sessions/{id}/events/poll
    Last-Event-ID: 41          ← 跟 SSE 完全同一个游标 header
    X-Poll-Wait-Ms: 25000      ← 可选，长轮询上限；缺省/0 = 立刻返回
→ 200 {"frames": [{"id": 42, "event": {...}}, ...], "next": 42}
```

**为什么不用 `?since=&wait=`**（原稿这么写，勘查后推翻）：`agent-server` 的 axum 是
`default-features = false, features = ["http1","json","tokio"]`——**没有 `query` feature**，
且 Cargo.toml 注释明确写「这个仓库没有查询参数协议」。为一个端点开这个口子不值得，
而 header 这条路**本来就有先例**：SSE 的 `Last-Event-ID` 就是这么读的（`routes/sse.rs`：
`headers.get("last-event-id").and_then(to_str).and_then(parse::<u64>)`，解析失败静默降级
`None`）。

**复用 `Last-Event-ID` 是这个设计最值钱的一点**：拉取式和 SSE 用**同一个游标语义**，
客户端在两种传输之间切换时游标逻辑零改动——这正是「同一个 ring 的两个投影」在协议面
上的兑现。

- `X-Poll-Wait-Ms` 缺省/0/解析失败 → 立刻返回（**纯轮询**），照 `Last-Event-ID` 同款静默降级
- 给了正数 → 没新帧就挂住最多这么久，有帧立刻返回（**长轮询**）。实现用
  `tokio::time::timeout` 包住 `live_rx.recv()`——注意 `agent-server/src` 现在**一次都没用过**
  `tokio::time::timeout`（唯一的定时器是 guard 的 `sleep`），这是第一次，别照抄测试里的用法
- 无 `Last-Event-ID` → 等价 SSE 首连：`replay(None)` = 「从缓冲区最旧那帧的前一个开始」，
  **必然是 Backlog、永不触发 Gap**（031 独测分歧 1 的裁决，ring.rs 已钉死）
- `next` = 最后一帧 id，也就是下次该传的 `Last-Event-ID`，**由服务端算**；空批保持传入
  游标（首拉无游标时为 `0`）。ring 只回 `id > Last-Event-ID`，所以 `next` 不能加一，否则
  会跳过下一帧。
- `Gap` 照 SSE 那条路同样合成一帧进 `frames`（标 `AgentId::root()`，`gap_frame_id = oldest-1`
  的自洽性已由 ring 保证），客户端语义一致

**一个端点覆盖轮询和长轮询**，差别只是那个 header。老的 `GET /events`（SSE）**保留不动**
——浏览器直连、桌面版都还在用，拉取式是**新增不是替换**。

### 断开检测：拉取式唯一的真缺口

SSE 有个隐藏福利：**客户端断开是免费可知的**。TCP 断了 → hyper 丢弃响应体 →
`SubscriberGuard` drop → 引用计数归零 → 宽限计时 → 取消在飞的轮次（不白烧 token）。
这套机制 031 的独测**专门踩过坑修过**：guard 必须活在 axum 会 drop 的那个 `Stream`
对象里，而不是活在只通过 mpsc 弱关联的后台任务里，否则「上游挂住 + 客户端断开」的组合
下永远发现不了断开。

**拉取式没有这个信号**——「客户端跑路了」和「它只是还没来拉下一次」在服务端看来一模一样。

**定的方案：每次 poll 期间持有一个 `SubscriberGuard`——整套机制原样复用，零新逻辑。**

原稿设计的是「last-poll 时间戳 + 自己写超时判断」。勘查完 `hub/guard.rs` 之后推翻——
**现有的引用计数 + 宽限倒计时恰好就是要的东西**：

```
attach（poll 请求进来）：subscribers += 1，abort 掉在飞的倒计时
drop  （poll 响应发出）：subscribers -= 1；归零才 tokio::spawn(sleep(grace))
                        → 到点二次确认 subscribers == 0 → handle.cancel()
```

于是拉取式的断开检测是**白拿的**：

- 长轮询挂住期间 guard 一直在 → 计数非零 → 不会误取消。
- 客户端在宽限内再来拉 → `attach` 的 `task.abort()` 顺手取消倒计时（**「是不是重连」不需要
  任何判断**，任何新连接天然满足——这正是现有实现的写法）。
- 客户端跑路 → 没有下一次 poll → 倒计时到点 → **走跟 SSE 断开完全相同的那条取消路**
  （`SessionHandle::cancel()`：先翻共享 `AtomicBool` 打断在飞 provider，再入队 `Command::Cancel`）。
- SSE 与拉取式**共用同一个计数器**：同一 session 上一个 SSE 观众 + 一个拉取网关，
  走掉一个不会误杀另一个。这是复用而非另起炉灶的额外红利。

**一条必须写进网关文档的约束**：短轮询（`X-Poll-Wait-Ms=0`）时 guard 在响应发出即 drop，
所以**客户端的轮询间隔必须小于宽限**（`DEFAULT_CANCEL_GRACE = 5s`），否则会被判成断开而
取消在飞轮次。**推荐网关一律用长轮询**（`wait` 取 20–25s）：guard 全程持有，既没有这个
约束，也顺带把空转请求降到最低。

**`POST /cancel` 仍是显式出路**，网关正常关闭时应该主动发；宽限是兜底（客户端崩了没人发）。
选宽限不选「纯靠显式 cancel」的理由：客户端崩溃/网络断是常态，只靠显式信号等于把「不白烧
token」这条正确性保证（ARCHITECTURE §取消传播：「这不是运维功能，是正确性」）交给调用方。

### 顺带捞到的一条：hub 表永不回收（静态分析发现，先写测试确认）

勘查传输面时发现一条**跟本里程碑无关、但被本里程碑放大**的问题，单列
[issue 059](issues/059-hub-leak.md)：

`SseHub::spawn` 里 `hub` 自持有 `handle: handle.clone()`，而 `SessionHandle` 内含
`events: broadcast::Sender<Frame>`；drain 任务又持有 `Arc<SseHub>`。于是：

```
drain task ──持有──> Arc<SseHub> ──持有──> SessionHandle ──持有──> broadcast::Sender
     ▲                                                                   │
     └──────── 等 sub.recv() 返回 None（需要所有 Sender 都 drop）◀────────┘
```

`sub.recv()` 返回 `None` 的**唯一**条件是所有 `Sender` 被 drop，而 drain 任务自己持有的那条
链上就有一个 Sender 克隆——**它在等的条件被它自己拿着的东西挡住了**。所以 session actor
死了之后 drain 任务照样不退出，末尾那句 `hubs.lock().unwrap().remove(&id)`（全 crate 唯一的
hub 清理点）永远执行不到：每个死会话泄漏一个 `SseHub`（含 256 帧 ring + broadcast channel）
和一个永久挂起的 tokio 任务。

**为什么现在才要紧**：单副本 + 会话数有限时它只是慢性累积；而 §三的 chatid 方案让**每个
业务 chat 都是一个 session**，量级完全不同。

**诚实标注**：这是**静态分析**的结论（引用链 + `recv` 的 `None` 条件），**没有实测**，
而且现有测试没有任何一条断言 hub 表被摘掉。所以 059 的第一步不是改代码，是**先写一条会红
的测试**（关掉 session → 断言 hub 表里那一项消失），确认了再修。

## 五、生命周期：Java 起子进程

```java
@PostConstruct void start() {
    readyFile = Files.createTempDirectory("agent-server-").resolve("ready.json");
    process = new ProcessBuilder(binPath, "--port", "0", "--sessions-dir", dir,
        "--ready-file", readyFile.toString())
        .redirectErrorStream(true).start();
    port = waitForReadyFile(readyFile).port();
}
@PreDestroy void stop() { process.destroy(); }   // Unix SIGTERM → Rust 侧优雅落盘退出
```

Rust bin 提供下面这组启动与关闭契约：

| 要什么 | 现成的 |
|---|---|
| 不跟 JVM 抢端口 | `--port 0` → 操作系统分配空闲端口 |
| Java 得知道实际端口 | `--ready-file <path>`：成功 bind 后原子发布 `{"port":…,"pid":…,"version":…}` |
| 停的时候别丢数据 | Unix SIGTERM/Ctrl-C 都走：**所有会话落盘快照之后才退**（`run.rs`） |
| 不对外裸奔 | `bind` 默认 `127.0.0.1`（红线 8），子进程天然只在本机 |
| 会话按 chatid 落盘 | `--sessions-dir <dir>` + §三的 chatid = 文件名 |

于是变成**一个部署单元**（jar + 一个二进制），Java 管起停/重启/健康。这正是**桌面版已经
验证过的模式**——Tauri 就是内嵌 server 绑 loopback 随机端口，前端一套不变只是 base URL 不同。

**代价诚实说**：要管二进制分发（按平台打进 jar 的 resources、启动时解压 + chmod +x）、
进程僵尸兜底、把子进程 stdout/stderr 接进 Java 日志。这些是 Java 侧的活，参考实现给样例。

`--ready-file` 是 Java↔Rust 的稳定启动协议，不要再解析启动横幅：Java 为每一次启动创建独占
目录并传一个尚不存在的文件路径，随后轮询该文件或同时检查子进程是否已退出。Rust 只会在
监听成功后以同目录临时文件 + 原子硬链接发布完整 JSON；如果发布失败就以非零退出。`pid` 可供
Java 与 `Process.pid()` 交叉校验，`version` 可供部署时做兼容性判断。

## 六、不做（延后或否决）

- **JNI / Panama FFI 真嵌入**（把 core 编译成 cdylib 给 Java 直接调）——**否决**，除非出现
  「必须单进程」的硬要求。代价里有几条是结构性的：现在的接口是 HTTP/JSON，要重新设计
  一整套 C ABI；**流式推送跨 FFI 很难做好**，而流式是这套东西的核心；**Rust panic 跨 FFI
  边界 = JVM 整个崩**（进程隔离没了，现在崩了只死那个进程）；agent-core 是 `!Send` 的单
  线程 actor，线程亲和性跨 JNI 要格外小心；再加每平台交叉编译与跨边界内存所有权。
  澄清：决策 12「企业内部服务也内嵌 `agent-server` 库」指的是**企业自己写 Rust 服务**去
  内嵌，不是 Java 内嵌。
- **WebSocket**——不做。这个场景服务端推为主、客户端只发少量命令（已有 POST 端点），
  双向不值得换来代理/网关配置的额外复杂度，而拉取式已经解决了 MVC 的问题。
- **网关侧共享拉取**（多个浏览器看同一 session 只拉一份）——延后。先「每连接一次拉取
  循环」，简单且够用；真出现同 session 多观众的负载再优化（ring 是共享的，改的只是网关）。
- **多副本粘性路由**——已有设计（ARCHITECTURE §多副本，`SessionRegistry` 抽象 +
  `RedisRegistry`），本里程碑不动。拉取式对它更友好（无长连接要粘）。

## 七、红线账

- **红线 8（bind 默认 loopback）**：子进程形态天然只在本机；新端点在 `agent-server` 下，
  不硬编码 `0.0.0.0`。
- **红线 11 不适用**：拉取式返回的是**网络协议面**，不进 prompt，不需要逐字节确定
  （区别于工具表）。但协议一致性（Rust↔TS 一份）仍由 032 的 ts-rs 生成 + 一致性测试锁。
- **红线 3 / 6 不碰**：不新增活句柄、不新增在飞 effect。ring 与 hub 都是既有的。
- **不新增第二真值源**：拉取式读的是同一个 ring。**不要**为拉取式另建一份缓冲——那就是
  两份事实，reconnect 时对不上（OBSERVABILITY §「snapshot 不是 reconstruct」同精神）。

## 八、issue 分解

- **055** chatid 幂等 getOrCreate + id 白名单校验（sonnet）：`POST /sessions` 接受指定 id、
  三态语义、路径穿越拒绝。独立可先发，不依赖拉取式。
- **056** 拉取式端点 `GET /events/poll`（sonnet，**独测 ✅**）：复用 `ring::Replay`、
  `next` 由服务端算、`Gap` 一致、长轮询的 `wait`。独测锁「拉取与 SSE 给出同一序列」。
- **057** 拉取式的断开检测（**opus**，独测 ✅）：每个 poll 全程持有 `SubscriberGuard`，
  直接复用 SSE 的计数、宽限与取消路。碰「不白烧 token」这条保证，且是时序相关的静默失败
  （宽限没生效 = 客户端跑了还在烧钱），
  031 的独测踩坑史是先例。
- **058** Java 参考网关升级 + 真机验收 ← M9 终点（sonnet + 主会话 dogfood）：网关改成
  拉取 Rust → 产生 SSE 给浏览器、`ProcessBuilder` 生命周期管理 + 端口握手、chatid 路由。
  Java 参考实现须通过 Maven 编译；真机验收还需带真实 provider 配置启动，确认浏览器
  断线、重连与多 tab 的完整链路。
- **059** hub 表永不回收（**opus**，独测 ✅）：见 §四末尾。跟 M9 无依赖关系，但被 chatid
  放大，**排在 055 之前做**——先写会红的测试确认，再断自持有那条引用链。
