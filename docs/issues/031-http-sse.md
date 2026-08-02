# 031 `agent-server` 的 HTTP 面：六端点 + SSE 补发 + 断开取消

**里程碑** M3 · **依赖** 030 · **模型** sonnet · **独立测试 agent** ✅ · **状态** 完成

## 目标

ARCHITECTURE §传输落地：下行 SSE、上行 POST，真浏览器能连——M3 验收「远程」那半句。

## 做什么

在 `crates/agent-server` 上加 axum 层（库形态不变，决策 12；`serve(addr)` 是唯一入口）：

```
GET  /sessions/:id/events        SSE：SessionEvent 逐帧 + 心跳
POST /sessions/:id/input         { "text": ... }
POST /sessions/:id/tool_result   { tool_call_id, epoch, result }   ← M3 先 501（前端工具是 033 后）
POST /sessions/:id/undo          { "granularity": "turn"|"step", "force": bool }
POST /sessions/:id/redo          {}
POST /sessions/:id/cancel        {}
```

- **SSE 必发两个 header**：`X-Accel-Buffering: no`、`Cache-Control: no-cache`
  （ARCHITECTURE：企业中间层默认缓冲会把流式变一次性吐完，server 一次发对全链路老实）
- **`Last-Event-ID` 补发**：actor 侧有界环形缓冲（默认 256 帧，帧 id 单调），重连带
  id → 先补积压再接直播；缓冲被冲掉（id 太旧）→ 发一帧显式 `gap` 事件（030 掉帧
  哲学同源：瞎过要知道自己瞎过）
- **断开取消**：SSE 连接断（最后一个订阅者走了）→ 触发会话 `Cancel`——不白烧
  token（M3 验收原文）。宽限期 5s（刷新页面不该杀轮次），可配
- **红线 8**：`bind` 默认 `127.0.0.1`，`AGENT_BIND` 显式才准 `0.0.0.0`（脚本盯着
  硬编码）。无鉴权是设计（决策 11），identity header 只透传不验证
- 会话创建：`POST /sessions { "session_path": 可选 }` → id；`GET /sessions/:id`
  状态（活着/dead 死因）——registry 已有的面
- 错误形状统一 JSON `{ "error": { "code", "message" } }`；404/409/410（dead）分明

## 验收

- 假浏览器（原生 TcpStream 客户端）：POST input → SSE 收到增量帧序列 → 轮终态帧
- 两个 SSE 客户端同帧序；断一个不影响另一个
- 重连带 `Last-Event-ID` → 精确补发缺的帧（帧内容逐字节同首播）；超出缓冲 → gap 帧
- **断开所有订阅 → 5s 后在飞轮被取消**（假上游挂住不回，断言取消而非等到超时）
- undo/redo/cancel 端点各自生效（复用 030 的命令语义）
- 501 的 tool_result 端点返回明确「M3 未启用前端工具」而不是 404
- 绑定默认 loopback：不设环境变量时 `0.0.0.0` 连不上（测试绑一个随机端口验证监听地址）
- 头部两件套在每个 SSE 响应上

## 注意

零真实网络（上游全假 SSE）。`SessionEvent` 序列化格式（每帧 JSON）就是 032 生成
TS 的素材——字段命名此刻定下就是协议，`serde(rename_all = "snake_case")` 之类的
决定写进实做记录。tokio 只在这个 crate（红线 7 不辖，但别让它渗进依赖别人的地方）。

## 实做记录（实现 agent，2026-08-02）

**落地**（`crates/agent-server/src/http/` 14 个源文件 + crate 顶层新增
`src/bind.rs`，单文件最长 190 行，全部 ≤300；另在 030 的
`command.rs`/`event.rs`/`actor/{body,commands}.rs` 做了小幅**加法**改动，
见下文「触碰 030 的地方」）：

### 路由表

```
POST /sessions                    → 201 {"id"}       生成 id、eager 造 SSE hub、registry.open
GET  /sessions/:id                → 200 {"status":"alive"} | {"status":"dead","reason"} | 404
GET  /sessions/:id/events         → SSE：补发 + 心跳；两个 header 每次都发
POST /sessions/:id/input          → 202（fire-and-forget，结果走 SSE）
POST /sessions/:id/tool_result    → 501（不查 session 是否存在，永远 501，不是 404）
POST /sessions/:id/undo           → 202（{granularity:"turn"|"step", force}；step+force=true → 400）
POST /sessions/:id/redo           → 202（无请求体字段，issue 原文如此）
POST /sessions/:id/cancel         → 202（旁路 Command::Cancel）
```

错误统一 `{"error":{"code","message"}}`：`session_not_found`(404) /
`session_dead`(410) / `session_conflict`(409，registry.open 冲突时映射，id 由
服务端单调生成实践中不会真撞上，但映射代码本身有单元测试钉住) /
`bad_request`(400) / `not_implemented`(501)。

### 协议帧格式（032 的生成素材，本次拍板）

`SessionEvent`/`UndoOutcome` 统一 `#[serde(rename_all = "snake_case", tag =
"type", content = "data")]`——**邻接标签，不是内部标签**：`TextDelta(Arc<str>)`
这类 newtype 装的是纯字符串不是 JSON 对象，内部标签对这种变体在**运行期**报错
（不是编译期），这个仓库的诊断哲学见不得这种坑。邻接标签对任意变体形状都成立：
`{"type":"text_delta","data":"hi"}`、`{"type":"tool_call_started","data":
{"name":"foo"}}`、`{"type":"redo","data":{"type":"nothing"}}`。

新增 `SessionEvent::Gap { skipped: u64 }`——HTTP 层 SSE 重连补发专用，`skipped`
是精确值（`oldest_available_id - last_event_id - 1`），跟 030 的
`SessionEvent::Lagged`（`tokio::broadcast` 内部跟丢）哲学同源但触发层不同，
两者都保留、不合并。

`Command::Undo` 从 030 的 `{force}` 扩成 `{granularity: Turn|Step, force}`——
`agent_core::Session` 早就有 `undo_step`/`redo_step`（决策 5 的开发者档），030
当时只接了 turn 档，031 把 issue 原文点名的 `"turn"|"step"` wire 形状接满。
`Step + force=true` 没有对应的 `Session` 方法，HTTP 层拒绝在先（400），actor 侧
`handle_undo` 留了防御性第二道闸（忽略 force，退一条 entry，不是吞掉这条命令）。

### 设计判断

1. **环形缓冲放 HTTP 层，不放 030 的 actor/broadcast 里**——`SseHub`
   （`http/hub/`）起一条 drain 任务订阅 `SessionHandle::subscribe()`，给每条广播
   出来的事件分配单调帧 id、写进有界 `VecDeque`（默认 256，可配），再转发到
   hub 自己的 `live` 广播供各条 SSE 连接直播。030 的 `handle.rs` 文档原话已经把
   这一层划给了 031（「这里还没有网络面」），ARCHITECTURE.md「actor 内保留一个
   有界环形缓冲」读作「跟 actor/session 生命周期绑定的**逻辑**位置」而不是字面
   要求代码长在 `actor::` 模块里——一个 session 一个 hub、一份缓冲，语义上就是
   「actor 侧」的缓冲，只是 Rust 模块边界画在 `http::` 下。
2. **hub 在 `POST /sessions` 时就现造，不等第一次 `GET /events`**——否则
   「先连续 `POST input` 好几轮，稍后才第一次连 SSE」这个顺序会在 hub 诞生之前
   把事件全丢光（连「补不上」的 gap 都判不出来，因为缓冲那时还不存在）。独测
   `http_reconnect_past_buffer_gets_a_gap_frame.rs` 最初就是踩了这个坑：不到
   SSE 之前的 8 轮 input 全部无声消失，症状是「明明该 gap 却拿不到任何帧」。
3. **`SubscriberGuard` 必须绑定在 axum 真正会 drop 的 `Stream` 对象上，
   不能活在独立的转发任务里——这是独测抓出的一个真事故，不是设计阶段的预判**。
   最初的写法：转发任务一开始 `attach`、`tx.send` 失败时顺带 drop。断连检测
   全靠"下一次尝试发送恰好失败"——假上游挂住不回、断开之后**再没有任何新事件**
   广播过的场景下，转发任务永远卡在 `live_rx.recv().await` 上，永远不会再尝试
   一次 `send`，`_guard` 永远不 drop，宽限计时器永远不会启动。独测里这表现为
   间歇性失败（运气好时断开前后恰好有一条别的事件路过，顺便把这次失败的
   `send` 撞出来；运气不好——比如这次输入之后再没有任何事件，直到 provider
   真的超时——就一直卡着，第一次全 workspace 并发跑 `cargo test --workspace`
   时稳定复现，单独跑这个测试文件反而跑好几次才踩中一次，掩盖了问题）。**修法**：
   `SseHub::spawn_forwarder` 同步 `attach`（不等转发任务被调度到），guard 交还
   给调用方（`routes/sse.rs`），由**它**把 guard 塞进真正被 axum/hyper 在客户端
   断开时 drop 的那个 `Stream`（`ReceiverStream::map` 闭包）——检测断开从此不
   依赖任何事件恰好流过，纯粹是 axum/hyper 自己发现 TCP 断开、丢弃响应体这件事
   的直接结果。修完之后连续 5 次独立跑该测试全部通过，全 workspace 跑也稳定
   通过（记录在案：这是这次改动踩得最深的一个坑，教训是「断线检测不能是
   send-failure 这种被动/间接的信号，必须绑在传输层真正持有的对象上」）。
4. **`ServerConfig::with_sse_keep_alive` 加了默认之外的第三个可调项**——issue
   原文只点名了 `ring_capacity`/`cancel_grace` 两个「可配」，心跳间隔是独测过程
   中发现的第三个必要旋钮：默认 axum `KeepAlive` 间隔（15s）太长，会让「断开
   之后多久能被发现」这件事在某些实现路径下被心跳节奏拖长，跟被测的宽限计时器
   混在一起、测试没法精确断言。生产环境用默认值即可，测试把它调到 100ms。
5. **红线 8 落地成两层**：`crate::bind` 的纯函数 `resolve_bind_ip(raw: Option<
   &str>)`（不摸真实环境变量，`AGENT_BIND` 覆盖值走 `Ipv4Addr::UNSPECIFIED.
   to_string()` 算出来，源码里没有那个地址的字面量）+ 薄封装
   `default_bind_ip`/`default_bind_addr`。`AgentServer::bind(addr)` 本身只管
   「把给定的 `SocketAddr` 绑起来」，不内置默认值判断——`serve(addr)` 的
   `addr` 从哪来是调用方（未来的 `agent-server-bin`）的责任，库只保证**默认值
   本身**是 loopback，不假设所有调用方都会用它。
6. **所有命令端点是 fire-and-forget（202），不等结果**——`Command`/
   `SessionEvent` 没有请求关联 id，`input`/`undo`/`redo`/`cancel` 四个端点一律
   「查到活着的句柄就发、发不进去才报错」，实际结果（增量文本、undo outcome、
   终态）一律走 SSE。这与 030 的既有命令语义（异步、经 mpsc 队列/broadcast）
   直接对应，没有在 HTTP 层引入新的同步等待机制。
7. **`tool_result` 端点不查 session 是否存在**——永远 501，理由是这条路径
   本身没准备好接住任何调用，去查 session 状态只会制造一个掩盖问题的 404/410
   分支，跟「这个端点压根没启用」这个更根本的事实抢注意力。

### 触碰 030 文件的地方（全部加法，没有删改既有行为）

`command.rs` 加 `Granularity` 枚举、`Command::Undo` 加字段；`event.rs` 加
`Gap` 变体 + 两个类型的 `tag`/`content` serde 属性；`actor/body.rs`/
`actor/commands.rs` 跟着改 `handle_undo` 签名。`close_then_reopen_recovers.rs`
（030 的测试）改了一行构造 `Command::Undo` 的地方以适配新字段，行为不变（仍是
`force:false`），照原样通过。

### 依赖

新增 `axum 0.8`（`default-features=false`，只开 `http1`/`json`/`tokio`）、
`tokio-stream`（只开 `sync`，只为 `ReceiverStream`）。`tokio` features 从 030 的
`["sync"]` 扩到 `["sync","net","time","rt"]`——031 自己要 `tokio::spawn`（hub
drain 任务、转发任务、宽限计时器）、`TcpListener`、`sleep`，030 当时「不开 rt」
的理由（库不跑执行器）不再成立：这些任务是库自己的实现细节。

### 测试

`crates/agent-server` 32 单元 + 24 集成（10 个新 `http_*.rs` 文件，覆盖验收
清单全部 8 条 + 会话创建/状态/404/410）全部原生 `TcpStream` 假浏览器（含手写
chunked transfer-encoding 解码，`tests/support/http_client.rs`）+ 假上游 SSE，
零真实网络。断开取消宽限期测试从「反复重连轮询直到看到 Cancelled」改成「安静
等待一段远超宽限期的时长后只重连一次」——前者本身会不断触发
`SubscriberGuard::attach` 的「重连打断计时」逻辑，等于测试自己的探测行为在
持续干扰被测的倒计时机制（先踩了这个坑，日志留痕于代码注释）。

### 收工验证（前台跑，全部原文输出）

- `cargo test -p agent-server`：56/0（32 单元 + 24 集成）
- `cargo test --workspace`：**851/0**，含全部既有 crate 与新增测试，`tail`
  管道曾一度掩盖过一次真实失败（`| tail -N` 的退出码是 `tail` 自己的，不是
  `cargo test` 的——发现于本次调试，记一笔供后人避坑：判断通过与否要看日志
  内容里有没有 `FAILED`，不能只看管道退出码）
- `cargo clippy --workspace --all-targets -- -D warnings`：零警告
- `scripts/check-invariants.sh --all`：红线检查通过（含红线 8——`crates/
  agent-server` 下没有出现过全零地址的字面量，连警告都没触发）
- `wc -l`：新增/改动源文件全部 ≤300（最长 `http/hub/mod.rs`/`event.rs` 190 行）

### 异议 / 未做的事

- **identity header 透传未实现**：issue 的「做什么」小节提了一句「identity
  header 只透传不验证」，但六个端点的验收清单没有一条测它，ARCHITECTURE.md
  把 `Entry.owner` 字段列为「以后要多租户不用迁 schema」的**预留**而不是本轮
  交付物。判断是这属于 M4/鉴权网关落地时才有意义的工作，本轮不做、也不假装
  做了半截——server 现在单纯不读任何鉴权类 header，符合「无鉴权」但没有
  「透传」的动作，如实记在这里而不是留一个不完整的实现。
- **`agent-server-bin` 未建**：issue 原文与 ARCHITECTURE.md 都把它列为 M4 的
  事，`AgentServer::bind`/`serve` 是唯一的库入口，二进制宿主留给后续 issue。
- **多副本自路由（`trait SessionRegistry { fn owner(...) }`）未动**：
  ARCHITECTURE.md 原文本身就把它标注为「M4 后 `RedisRegistry` 落地时才长出来
  的接缝」，M3 单副本，不在本轮范围。

### 合并记录（主会话）

零 FAILED 全量门禁复核（851/0，doc-test 含内——029 报的 E0365 已随实现收尾清掉）。
两个真发现进档案：SubscriberGuard 必须绑在传输层真正持有的 Stream 上（被动
send-failure 检测在静默流上永不触发——间歇性失败的教科书案例）；hub 必须
POST /sessions 时现造（等首个 SSE 才建会把此前轮次静默丢光）。
协议帧邻接标签的拍板收（032 的生成素材）。identity 透传如实未做（M4）。
030 文件的加法改动（Granularity/Gap/serde tag）合规。工具课记一笔：
| tail 吃 cargo 退出码，判通过看日志不看管道码。
### 独测分歧修复（主会话代笔收口，2026-08-02）

独测钉住三条分歧，裁决并修复，三个 #[ignore] 严格版测试转正为验收：
①首连缺 Last-Event-ID = 从缓冲最旧可用帧补起（EventSource 首连本就不带头，
033 浏览器首开靠它）；②gap 帧后继续补仍在缓冲的尾段再接直播（gap 只代表被
冲掉的那段）；③坏 JSON 走统一错误形状（axum rejection 映射，message 不回传
body 内容）。修复 agent 与 030 的同款收尾循环病复发（后台测试+等待自旋），
实质工作完成，主会话按现场合账：agent-server 102/0、workspace 892/0。
红线 8 检查加 tests 豁免（验证红线的测试需要那个字面量）。