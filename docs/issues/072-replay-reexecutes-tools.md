# 072 重连/刷新会让前端把历史里的工具调用**再执行一遍**

**里程碑** M10 阻塞项（068 真机前必须有答案） · **依赖** — · **模型** opus · **独测** ✅

066 落地时发现并如实上报的一条**协议级缺口**。示例工具都是 `pure` 所以现在无害，
但 M10 的整个目的就是让宿主注入**真实业务工具**——那时它就是重复下单。

## 现象

1. 模型调 `web:crm/create_order` → server 推一帧 `tool_executing` → 前端执行 → 回传 → 完事。
2. 一个**新的客户端实例**接到**同一个已有历史的会话**上 → 拉/推历史帧，其中就有那条
   `tool_executing`。
3. 它**无法区分**「这是补发的历史，已经做过了」和「这是刚派给我的活」——
   `tool-exec.ts` 里的 `handled: Set<call_id>` 是**进程内**的，新实例是空的。
4. 于是**再下一次单**。

### ⚠️ 第 2 条原来写的是「用户刷新页面」——**那是错的，主会话核实后改的**

这条 issue 是 066 落地时写的，当时没核 `createSession` 的签名。事实（带行号）：

- `packages/web/src/api.ts:51` 的 `createSession(capabilities?)` **没有 chatid 参数**；
  `main.ts:18` 每次开页都建**全新会话**，session id 不落 localStorage。
  **刷新 = 新 session = 空 ring，没有历史帧可重放。**
- 同一次开页内 SSE 断线重连也不出事（`connection.ts:19` 的原生 `EventSource` +
  `:28` 的水位去重）——**为什么不出事见下面第 1 条，根因在服务端不在前端**。

**所以这条在 web demo 上复现不了，别去浏览器里试。** 它要的是「同一个 chatid 上换一个
新客户端实例」——那正是 **055 的 chatid 幂等 getOrCreate + M9 网关拉取式**那条路，
也就是 M10 企业集成的正主。记这一笔是因为：照着「刷新页面」去复现会失败，
然后得出「这 issue 是假的」——比没有复现步骤更贵。

**两个被点名要核的面，核完了（都带行号）：**

1. **同一次开页内的 SSE 自动重连：带 `Last-Event-ID`，不是爆炸半径。** 根因在服务端——
   每帧都发了 `id:`（`routes/sse.rs:58` 的 `Event::default().id(frame.id.to_string())`），
   浏览器于是存得下 `lastEventId` 并在原生重连时带回来 → `replay(Some(last))`
   只给 `id > last` 的帧（`ring.rs:97`），**已经见过的一帧都不会再来**。`Gap` 那一支
   也安全：`tail` 里全是客户端没见过的帧（`ring.rs:92-96`）。加上 `createToolExecutor`
   只在 `main.ts:26` 调一次、重连不重建，`handled` 还在——**服务端游标 + 客户端集合，
   两道保险**。
2. **拉取式（网关那条）：游标一断就是全量重放，这才是正主。** 链条逐环都核了：
   网关按 chatid 走 055 的幂等 `getOrCreate`（`AgentSessionClient:29-37`）→ 会话与 ring
   长期活着；**浏览器刷新 = 新 `EventSource` = 没有 `Last-Event-ID`** →
   `AgentSseController:35` 的 `parseCursor(null)` → 每条浏览器连接一个自己的
   `PollCursor`（`:95-116`，进程内、不持久）→ 空游标 poll Rust → `replay(None)` →
   **整个 ring 重放**，那条 `tool_executing` 原样再来一遍。网关重启 / 新 tab 同理。

**server 侧不会出错**：那次迟到的回传会被 `take_remote_tool` 按 `(agent, call_id)` 找不到
而安全拒绝（既有 `TransportTrouble` 路）。**坏的是副作用已经在宿主侧真的发生了第二次。**

## 为什么现在必须解决

M10 之前，`Location::Web` 的工具只有 `ask_user_question`/`browser_action`/`save_file`
三个裸名工具，前端根本没接执行（`render/tool.ts` 只画卡片）。**066 第一次让前端真的执行**，
而 M10 的卖点就是宿主注入自己的业务能力——`pure` 是特例，`irreversible` 才是常态
（061 的缺省就是 `Irreversible`）。

## 拍板（opus，2026-08-04）：**待办投影——帧只是触发器，服务端的等待槽才是判据**

**「这帧是不是补发的」根本不是正确的判据。** 派了活、前端还没执行就刷新 → 那帧确实是
补发的、活却真的还欠着 → 按「补发就跳过」办 = 这活永远没人干。**唯一权威的判据是
「这次调用现在是否还在 `PendingRemoteTool` 里等着」**——那是服务端状态，刷新不掉。

于是三条规则：

1. **服务端把等待槽导出成一份只读投影**：`GET /sessions/{id}/pending_tools` → 此刻还欠着
   的 `(agent, call_id, request)` 列表。数据源是 `RunnerCtx` 里那张表本身，**不是第二份账**。
2. **前端收到 `tool_executing`（`location === "Web"`）时不再直接执行**，先向这份投影求证，
   命中才跑。帧退化成「去问一下」的触发器。
3. **每次连上（首连 + `EventSource` 每次自动重连）拉一次投影，把里面的活补执行掉**——
   顺手覆盖「帧根本到不了」的两种情况：ring 被挤爆（`Gap`，那帧永久没了）和断线期间派的活。

这跟 M9 把拉取式定义成「ring 的第二个投影」是同一种做法：**权威在状态，传输只是投影**
（INTEGRATION.md §四）。也跟 048/049 的活树逐条同款：`GET /agents` 做种 + 帧增量更新，
**推和拉两条路给出同一份事实**。

### 核实到的代码事实（这次勘查，带行号）

| 事实 | 位置 |
|---|---|
| 等待槽长什么样、活在哪 | `agent-runtime/src/ctx_remote_tools.rs:18-31`（`PendingRemoteTool{agent,call_id,epoch,request,deadline}`），`RunnerCtx` 私有字段在 `ctx.rs:81` |
| 它的**全部**变更点只有四个，且全在同一个文件 | `register_remote_tool:39-56` / `take_remote_tool:59-70` / `take_expired_remote_tools:74-85` / `discard_remote_tools:103-105` |
| 已经公开出去的只有两个纯读函数 | `next_remote_deadline:92`、`pending_remote_tool_count:98`（060 加的） |
| 跨层**不是没有通路，是有现成通路没用**（048 的先例） | `SessionHandle.tree: Arc<Mutex<AgentTree>>`（`agent-server/src/handle.rs:94`）：actor 起来时种（`actor/body.rs:89`）、`with_tree_events` 回调每次变了重写（`body.rs:155-158`）、`GET /sessions/:id/agents` 直接读（`routes/sessions.rs:216-219` → `handle.rs:126-128`），**不排 mpsc 队列** |
| ring 对帧**确实**一视同仁 | `hub/ring.rs:56-65` 的 `push` 不看帧内容；`replay:83-99` 只按 id 过滤；三态在 `:30-46` |
| `tool_executing` 怎么产生 | 远端第五路 `agent-runtime/src/dispatch.rs:142-146`：**先 `register_remote_tool`（143）再 `emit`（144）**——本方案的地基就是这个顺序 |
| 同一个事件另有五处产生者 | `tool_exec.rs:23`（本地）、`dispatch.rs:177`（MCP）、`spawn_tool.rs:214`、`status_tool.rs:107`、`collect_tool.rs:101`。所以**帧本身不携带「这次要你执行」**，前端只能靠 `request.location` 猜（`tool-exec.ts:68`） |
| 前端 `handled` 怎么用 | `packages/web/src/tool-exec.ts:63`（每次 `createToolExecutor` 一份）、`:71-73` 判重即标记。`FrameWatermark`（`dedupe.ts:9-22`）同样是进程内。**刷新两者都归零**——`tool-exec.ts:25-35` 已经把这个缺口如实写在文件头 |

### 四个候选：逐条说为什么不是它

**候选 1（server 重放时过滤/改写「已经收场的」帧）——否。** 两条独立的硬伤：

- **它让 ring 不再是日志。** 同一个 `Last-Event-ID` 在不同时刻重放出不同内容，
  056 的「拉取与 SSE 给出同一序列」从此是时序相关的（`hub/mod.rs:196-200` 与
  `routes/poll.rs:35,76` 读的是同一把锁下的同一份 ring）。INTEGRATION.md §七 那条
  「**不新增第二真值源**」的反面：这是把唯一的真值源改成会变的。
- **它要的事实恰好就是拍板方案那份投影，区别只在拿它干什么。** 候选 1 的判据不是
  「是不是补发」（那是候选 2 的病），而是「还在不在槽里」——**判据本身是对的、跟本方案
  同源**。差别在：本方案把这份事实**作为独立投影暴露**，候选 1 拿它去**改写日志**；
  前者什么都不损失，后者损失一个日志，并把「要不要执行」这个**客户端的决定**编码进了
  服务端下发的历史（渲染层想画"当时确实调过"、审计在别处重放同一段，拿到的都是被裁剪的过去）。

**候选 2（帧标 `replayed: true`，前端只执行非补发帧）——否，且它单独用是错的。**

- **判据本身就错**：见本节开头。刷新时仍在等待的调用会被跳过 = 漏活，
  比重复执行更隐蔽（本 issue 验收第二条就是为它写的）。
- **它在唯一能复现这条 bug 的那条路上恰好完全失效**（爆炸半径核实之后这条从"缺点"
  升级成"致命"）：poll 端点的帧**全部**来自 ring（`poll.rs:35`/`:76`），服务端没有
  live/补发这个区分可用 —— 标成 `replayed:true` 则网关背后的浏览器一件活都不干，
  标 `false` 则等于没做。而**能中这条 bug 的客户端，正好全都在网关背后**。
- 附带代价（原文已列，仍然成立）：`Frame`/`SessionEvent` 形状变了 → ts-rs 生成 +
  `sample_events_cover_every_variant_at_least_once` 的「16」+ 所有消费者。

**候选 3（前端持久化 `handled`）——否，只可作加固；爆炸半径核实之后它比原文判得更死。**
原文的理由（换浏览器/清缓存/隐私模式全失效、每个宿主各实现一遍）不变，再加一条：
**能中这条 bug 的客户端是一个开放集合**——今天是浏览器和 Java 网关（JVM 里没有
`localStorage` 这套东西），明天是我们没见过的第三种宿主。正确性边界不能放在一个
「每加一个集成方就要重新实现一遍、而且漏了不报错」的地方。**修法必须在服务端成立**，
客户端那一侧只该剩一条薄规则（执行前问一句）。

**候选 4（要求工具自带幂等键）——否。** 把责任推给每个集成方，而 061 的可逆性声明
已经表明我们知道哪些工具不安全。可以**叠加**（宿主愿意做幂等更好），但不能当解法。

### 它怎么同时满足两条验收

- **不重复执行**：刷新后那条 `tool_executing` 照旧被补发，但前端拿它去问投影——
  那次调用早已被 `take_remote_tool`（`ctx_remote_tools.rs:59-70`）取走，投影里没有，
  **不执行**。判据是服务端状态，跟"这帧是第几次见到"无关。
- **不漏活**：派了活还没干就刷新 → 槽还在 → 投影里有 → **执行**。而且比今天更强：
  连帧被 ring 挤掉（`Gap`）都不会漏，因为规则 3 是从投影拉，不依赖帧还在不在。

### 落地形状（只写不改）

| 文件 | 大概做什么 |
|---|---|
| `agent-runtime/src/ctx_remote_tools.rs` | 四个变更点各调一次投影回调；加 `pub struct RemoteToolWaiting{agent,call_id,request}` 与 `RunnerCtx::pending_remote_tools()`。`impl RunnerCtx` 本来就在这个文件里 |
| `agent-runtime/src/ctx.rs` | **只加一个字段 + 它的文档**。这个文件 289/300，只剩 11 行余量——builder（`with_pending_events`）放上面那个 `impl` 里，别放这儿（红线 9） |
| `agent-server/src/handle.rs` | `SessionHandle` 加一个共享单元格 + 读方法，照 `tree`（`:94`/`:126-128`）逐行同款 |
| `agent-server/src/actor/{mod,body}.rs` | 造单元格（`mod.rs:82` 同款）+ 接回调（`body.rs:155` 同款） |
| `agent-server/src/http/routes/pending_tools.rs`（新）+ `routes/mod.rs` | 第 11 个端点。死会话 410，跟 `agents` 同一条判据（`state.session_handle`） |
| `agent-server/src/http/pending.rs`（新） | wire 类型，位置照 `poll_protocol.rs`；ts-rs 门在 `ts` feature 后 |
| `packages/web/src/api.ts` | `fetchPendingWebTools(id)` |
| `packages/web/src/tool-exec.ts` | 判据换成求证；文件头 `:25-35` 那段「不归本文件管的已知面」改写成「归它管了」 |
| `packages/web/src/main.ts` | `state === "open"` 那个钩子（`:38`，首连和每次重连都会走）再挂一次「拉待办 → 执行」 |
| `packages/protocol/src/{generated/*,index.ts}` | 重新生成 + 按既有规矩收拢导出 |

**写点必须在「槽变化的那一刻」，不能在 actor 的命令边界。** `register_remote_tool` 发生在
`run_turn` **内部**（`dispatch.rs:143`），帧在下一行就广播出去了；投影若等命令处理完才刷新，
客户端就有一个「收到帧、去问、说没有」的窗口 → **漏活**。`emit_tree_snapshot` 那形状在这儿是错的。

### 协议面变更

- **新增一个只读端点 + 一个新 wire 类型；`Frame`/`SessionEvent` 一个字节不动**——既有消费者
  （渲染层、Java 网关、fixtures）全都不用改，`SessionEvent` 变体数还是 16
  （`ts_protocol/consistency.rs:122-129` 不动）。这是选它而不是候选 2 的直接红利。
- ts-rs：`ts_protocol/export.rs:39-52` 加一行 `export_all`，重新生成 `packages/protocol/src/generated/`。
  `generated_ts_matches_committed_snapshot`（`consistency.rs:85-96`）会强制这件事被做完，
  **两件事必须同一次提交**。
- **Java 网关零改动**：`AgentProxyController` 的 `/agent/**` 通配已经透传任意短请求，
  新端点白拿；SSE 那条路不碰。
- 红线 11 不适用（走协议面不进 prompt）；红线 3/6 不碰（不新增活句柄、不新增在飞 effect、
  投影是纯读，epoch 一个字不动）。

### 跟 060 的互动：两种拆台，都要避开

060 的两半是 `deadline::sweep`（泵里）+ `sweep_remote_tool_deadlines`（宿主侧，
`deadline.rs:67`；actor 空闲时靠 `next_remote_deadline` 决定 `recv_timeout`，
`actor/body.rs:208-228`）。

1. **别让 072 把活丢给 060 兜。** 如果按候选 1/2 那样「补发就跳过」，那条活其实还在槽里——
   060 会在**十分钟后**把它判失败注入 `is_error`。会话不挂死（060 兑现了它的承诺），
   但用户刷新那一刻现场什么都看不出来，工具没跑、模型干等十分钟。这就是"漏活比重复执行更
   隐蔽"的具体形状：**有兜底反而更难发现**。
2. **别让 060 把已经收场的调用留在投影里。** 截止线到点走的是
   `take_expired_remote_tools`（`ctx_remote_tools.rs:74-85`），投影必须在这一刻同步收缩；
   漏了这一处，前端刷新后会执行一个**已经按失败收尾**的调用——回传照旧被安全拒绝，
   副作用照旧发生，正是本 issue 的病换个入口复发。`discard_remote_tools`（取消/undo/redo，
   `commands.rs:62,84,98`）同理。

### 跟 062/073 的冲突面

- **语义上不冲突。** 073 管「注入的声明进 store、恢复时原模原样复刻」，那是**会话状态**；
  等待槽是**运行时状态**（`RunnerCtx`，不落盘、不跨进程）。两者各管各的。
- **文本上只有一个文件会撞**：`actor/body.rs`（062/073 在那里装配 `ToolTable`，本方案在那里
  接回调）。所以 **Rust 侧排在 062/073 之后做**，或至少 rebase 时只处理那一段。
- **跨进程不在射程**：actor 死了槽就没了；恢复出来的会话若正停在 `ToolsPending`，既没有槽
  也没有截止线（`next_remote_deadline()` 是 `None`）——**未核实的邻接面，值得单开一条**。

### 诚实的代价与残留

- **每次远端工具调用多一次 GET**（本机/网关短请求，相对模型往返和工具本身可忽略）。
  可选优化（**不做，留档**）：响应带一个 `as_of` 帧 id，只对 `id <= as_of` 的帧求证；
  它要求服务端**先读 ring 头 id 再读待办表**（反过来会漏活）——为省一次往返引入一条
  要证明的时序规则，不值。
- **多客户端：治大头，不治并发的那一格。** 同 chatid 新开一个 tab 跟"刷新"是**同一个
  爆炸半径**（无游标的新客户端 + 有历史的 ring），所以本方案连它一起治了：历史里**所有
  已经收场的**调用一条都不会重跑。**剩下的那一格是并发**——两个客户端同时收到同一条
  **仍在等待**的调用，两边求证都说"还欠着"，于是各执行一次。真要 exactly-once 得把
  "认领"变成状态变更（`POST /tool_claim` 走命令队列 + 应答），而 `Command` 现在是
  fire-and-forget、没有应答通道（`http/state.rs:82-85`）。备案在此，不在本 issue 做——
  但**要写进宿主契约**：一个 chatid 同时挂多个执行端，那一格今天没人兜。
- 求证到执行之间槽被别人收走（TOCTOU）仍会执行一次，同上一条，毫秒级窗口且只在多客户端下存在。

## 验收（可判定）

- **重放不重复执行**：建会话（**指定 chatid**）→ 模型调一个 `web:` 工具 → 客户端执行并
  回传 → **换一个没有游标的新客户端实例接上同一个 chatid**（新建一条 SSE/poll 连接、
  不带 `Last-Event-ID` 或带一个更早的）→ 断言**没有第二次执行**（用一个带副作用计数的
  假工具，断言计数 == 1）。原文这里写的是"模拟刷新页面"，**按上面核实的结果改成了
  "同 chatid 换新客户端"**——刷新只是它在网关那条路上的一个实例，照字面去 demo 里刷新
  测不到东西。**这条在修之前必须是红的**——先写它、跑一次看红、再修（059 的先例）。
- 正常的「派了活还没干」路径不受影响：重连之后**仍在等待中**的调用要能被执行
  （否则会把活漏掉，比重复执行更隐蔽）。这条要有对照断言。
- **投影跟槽同生同灭**：回传/超时/取消之后，投影里那一条**立刻**没了（服务端侧断言）。
- **ring 没被动过**：新开一条不带 `Last-Event-ID` 的连接，那帧 `tool_executing` 照样原样补发
  ——候选 1 被否掉的理由要有测试守着。
- 既有远端闭环不回归：`web_tool_result_resumes_turn.rs`、`web_tool_never_answered_times_out.rs`、
  066 的 `verify:tool-exec` 23 条。
- **真机验收（068）走网关那条路，不走 demo**：demo 每次开页新建会话（`main.ts:18`），
  **碰不到**这条 bug，也就证明不了修好了。备选是给 demo 加「固定 chatid」开关
  （`?chat=<id>`），但那会改变 demo 的产品行为（刷新 = 续聊），**不在本 issue 射程**。

## 「先红」那条测试怎么构造

**落点**：`packages/web/scripts/verify-tool-exec.ts` 新增第 [8] 组——bug 在前端行为上，
断言就该落在前端。那个文件已经有 mock server（`:60-88`）逐条复刻服务端契约的做法。

**mock 补两件事**：一张 `pending: Map<call_id, {agent, request}>`（测试自己放/删），
和一个 `GET /sessions/:id/pending_tools` 读它。收到 `POST /tool_result` 时把对应项删掉
——这就是"server 侧那次调用已经收场"。

**假工具**：`registerWebTool("web:verify/counted", …)`，实现体**第一行** `sideEffects += 1`。

**剧本**：

1. mock 标 `call-refresh` 为 pending；`page1 = createToolExecutor("s-1")`；喂
   `frame("call-refresh","web:verify/counted")`；等回传到达；断言 `sideEffects === 1`；
   mock 收到回传后把它从 pending 删掉。
2. **模拟「无游标的新客户端」**（刷新 / 新 tab / 网关重启在前端这一层长得一模一样：
   一份没有任何记忆的新实例）：`const { createToolExecutor: fresh } = await
   import("../src/tool-exec.ts?reload=2")`，`page2 = fresh("s-1")`。
3. 喂**同一帧**——这正是 `replay(None)`（`ring.rs:83-99`）会给它的东西。
4. 等 300ms（给它足够时间去犯错），断言 `sideEffects === 1` 且 mock 没收到第二条回传。
5. **对照组（不漏活，跟第 4 步同等重要）**：mock 标 `call-owed` 为 pending 且**永不删**；
   `page2` 喂那一帧；断言 `sideEffects === 2` 且 mock 收到 `call-owed` 的回传。
6. **第三条（Gap 那一路）**：只往 mock 的 pending 表里放 `call-gap`、**一帧都不喂**，
   走一次「连上就拉待办」的入口，断言它也被执行。

**服务端另配一个测试文件**（`agent-server/tests/http_pending_remote_tools_projection.rs`，
骨架照 `web_tool_result_resumes_turn.rs`，但**建会话时指定 chatid**——`POST /sessions
{"id":"chat-072"}`，055 那条路），三条：

1. **病因证据（今天就绿，不是修复目标）**：拿到 `ToolExecuting`、`POST /tool_result`
   收场之后，**新开一条不带 `Last-Event-ID` 的连接**，断言那帧 `tool_executing`
   **真的又来了一次**。这条把「爆炸半径在 chatid 那条路上」钉成可执行的事实，
   而不是一句文档里的话；顺带守住「我们没去偷偷改写 ring」（候选 1 被否的理由）。
   **SSE 和 `/events/poll` 各断一次**——网关走的是后者，只测 SSE 等于没测正主。
2. **投影跟槽同生同灭**：`ToolExecuting` 之后 GET 投影精确断言只有那一条
   （agent/call_id/tool 都对得上）；`POST /tool_result` 之后再 GET，断言**空**。
3. **超时那一路同款**：压短 `remote_tool_timeout`（照 `web_tool_never_answered_times_out.rs`
   的 300ms），到点之后 GET 投影断言**空**——`take_expired_remote_tools` 那一处漏了
   就是本 issue 的病换个入口复发（见 §跟 060 的互动 第 2 条）。

### 为什么这条测试一定会红（突变论证）

- **今天就是红的，不需要任何新产品代码**：`handled` 是每个 `createToolExecutor` 一份
  （`tool-exec.ts:63`），`page2` 的集合是空的 → 第 3 步直接执行 → 第 4 步 `2 !== 1` 炸。
- **突变 1**：修好之后把 `tool-exec.ts` 里那次求证删掉（回到直接执行）→ 第 4 步炸。
- **突变 2**：把判据取反（"不在待办里才执行"）→ 第 5 步对照组 `sideEffects` 停在 1、
  mock 收不到 `call-owed` 的回传 → 炸。
- **突变 3**（服务端侧）：把投影的写点从 `take_remote_tool` 挪到命令边界 → Rust 那条
  「回传之后投影为空」变红。
- **为什么第二道防线兜不住它**：计数发生在假工具实现体的第一行，也就是真实世界里"下单"
  那一行。服务端拒不拒这次迟到的回传（`take_remote_tool` 找不到 → `TransportTrouble`）
  跟这个计数完全无关。本仓上次白写的那条测试栽在"断言的是下游可观测的结果，而下游还有
  一层兜底"；这一条断言的是**上游的副作用本身**，它之后没有任何东西能把它撤销。
- **为什么必须重新 `import`、而不是只 new 一个 executor**：只 new 一个 executor 的话，
  "把 `handled` 提到模块级单例"这种**错的修法**能骗过测试（Node 进程内模块状态不会因为
  new 一次就没）；真刷新连模块状态一起没。带 query 的动态 import 让 Node 重新求值那个模块。
  （`scripts/ts-resolve.mjs` 的钩子先试 `nextResolve(specifier)`，带扩展名的
  `../src/tool-exec.ts?reload=2` 会直接命中；万一 Node 的类型擦除不认带 query 的路径，
  就在钩子里剥掉 query 再解析——**改测试脚手架，不改产品代码**。）
- 同一条构造也顺手挡住候选 3：Node 里没有 `localStorage`（要
  `--experimental-webstorage`），押在客户端存储上的实现在这条测试里根本跑不起来。

## 注意

- **这条和 060 是一对**：060 解决「派了活没人干 → 挂死」，本 issue 解决「同一份活被干两次」。
  两条怎么互相拆台，见上面那一节，**别只看这一句就动手**。
- **不要选方向 3 当主方案**（客户端存储不是正确性边界），可以作为额外加固。
- 协议一致性由 032 的生成 + 测试锁：改 `export.rs` 和重新生成 `packages/protocol/` 必须同一次提交。
- **红线 9 有具体的坑**：`ctx.rs` 289/300、`body.rs` 228、`handle.rs` 221——加东西前先看
  余量，`ctx.rs` 只放字段。
- **别再按「刷新页面」去理解这条 issue**：那个前提已被核实推翻（§现象 的 ⚠️ 块）。
  判据是「会话身份被复用 + 客户端没有游标」，触发者是网关/多 tab/网关重启。
  拍板方向不受影响——它本来就没依赖「这帧是不是补发」，反而因为爆炸半径落在网关那条路，
  候选 2 从"有缺点"变成"在正主路径上完全失效"、候选 3 从"不该选"变成"结构上不成立"。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（2026-08-04）

按拍板方案落地，**没有偏离**：新增一个只读端点 + 一个 wire 类型，`Frame`/`SessionEvent`
一个字节没动（`SessionEvent` 变体数仍是 16，`ts_protocol/consistency.rs:125` 那条没改）。

### 投影挂在哪

数据源是等待槽表**本身**，一路只做投影、不做第二份账：

| 层 | 东西 | 位置 |
|---|---|---|
| runtime | `pub struct RemoteToolWaiting{agent,call_id,request}` + `RunnerCtx::pending_remote_tools()` + `with_pending_remote_tools(cb)` + 私有 `publish_pending_remote_tools()` | `agent-runtime/src/ctx_remote_tools.rs`（`impl RunnerCtx` 本来就在这个文件里） |
| runtime | 只加一个字段 `on_pending_remote_tools`（+ 文档，**builder 没放这儿**） | `agent-runtime/src/ctx.rs`（289 → 295 行，红线 9 还剩 5 行） |
| runtime | 跨层出口 | `agent-runtime/src/lib.rs` 的 `pub use ctx_remote_tools::RemoteToolWaiting`（模块本身仍私有） |
| server | `SessionHandle.pending_tools: Arc<Mutex<Vec<RemoteToolWaiting>>>` + `pending_remote_tools()` | `agent-server/src/handle.rs`（照 048 的 `tree` 逐行同款，**不排 mpsc 队列**） |
| server | 造单元格 + 传进线程 | `agent-server/src/actor/mod.rs:88`（照 `tree` 同款；空 `Vec` 是**真实**初值不是占位，所以不像 `tree` 那样需要在握手前覆盖一次） |
| server | 接回调 | `agent-server/src/actor/body.rs`，紧跟 `with_tree_events` 之后。**只重写单元格，不广播帧**——投影是判据不是时间线上的事，`tool_executing` 那一帧派活时已经发过了 |
| server | wire 类型 / 端点 | `agent-server/src/http/pending.rs`（新）、`.../routes/pending_tools.rs`（新，第 11 个端点） |
| 协议 | ts-rs 一行 + 重新生成 | `ts_protocol/export.rs:47`、`packages/protocol/src/generated/PendingTool{,sResponse}.ts` + `index.ts`（同一次做完，`generated_ts_matches_committed_snapshot` 绿） |
| 前端 | `fetchPendingTools(id)` / 判据换成求证 + `sweep()` / 连上就扫 | `packages/web/src/{api,tool-exec,main}.ts` |

**写点在「槽变化的那一刻」，不在命令边界**：`publish_pending_remote_tools()` 就在
`ctx_remote_tools.rs` 那四个变更点内部，`register_remote_tool` 那一次发生在
`dispatch.rs:143`——**下一行**就 `emit` 了 `tool_executing`。所以客户端拿着帧立刻来问，
问到的必然已经是含这条调用的新投影，没有「收到帧 → 去问 → 说没有」的窗口。

### 四个变更点全接上了，且各有一条测试守着

`agent-server/tests/http_pending_remote_tools_projection.rs`（建会话**指定 chatid**
`chat-072`，055 那条幂等路）。逐点做过突变（把那一处的 `publish_` 换成注释再跑）：

| 变更点 | 语义 | 守它的测试 | 突变掉之后 |
|---|---|---|---|
| `register_remote_tool` | 派活 | 三条都断言「刚派出去投影里该有它」 | 3 条红（`派出去的调用该在投影里：{"pending":[]}`） |
| `take_remote_tool` | 正常回传收场 | `..._empties_on_the_result` | 1 条红（回传后投影里那条还在） |
| `take_expired_remote_tools` | 060 截止线判失败 | `..._when_the_deadline_takes_the_slot` | 1 条红 |
| `discard_remote_tools` | 取消 / undo / redo | `..._when_the_turn_is_cancelled` | 1 条红 |

最后两条正是 §跟 060 的互动 第 2 条点名的「同一个病换个入口复发」：漏了它们，宿主刷新后
会去执行一个**已经按失败收尾 / 已经被斩断**的调用。取消那一条是本次**在 issue 原文三条
之外补的第四条**——原文的三条没有覆盖 `discard_remote_tools`，突变会活下来。

### 「病因证据」那条：今天就绿，且**跟修复完全无关**

它不碰新端点一步，只做「收场 → 换一条没有游标的连接 → 那帧原样又来一次」。四次服务端
突变（含把 `register` 的写点整个拿掉 = 服务端等于没实现 072）它**全程 `ok`**，证明它断言
的是 bug 的前提而不是修复本身；修完继续绿，于是它同时守住「我们没去偷偷改写 ring」
（候选 1 被否的理由）。真实输出（`--nocapture`）：

```
[072 病因证据] 无游标的新 SSE 连接补发到的帧：id=Some(7) {"agent":"root","event":{"type":"tool_executing","data":{"call_id":"call_browser_1","request":{"tool":"browser_action","input":{...},"location":"Web","reversibility":"Irreversible"}}}}
[072 病因证据] 无游标的 /events/poll 拿到的整批：{"frames":[{"id":1,...},...,{"id":7,"event":{"agent":"root","event":{"type":"tool_executing",...}}},{"id":8,...}],"next":14}
```

**SSE 和 `/events/poll` 各断一次**（网关走后者）。注意那个 `"reversibility":"Irreversible"`
——`browser_action` 本来就不是 `pure`，重跑一次的代价是真的。

### 前端那条「先红」的真实输出

`packages/web/scripts/verify-tool-exec.ts` 新增第 [8] 组。**动实现之前**跑一次：

```
[8] 072：重放不重复执行——判据是服务端的待办投影，不是「这帧是第几次见到」
  —— [8.1] 派了活 → 执行 → 回传 → server 侧收场
  ✓ 第一次派发真的执行了（副作用计数 1）
  ✓ mock 侧那条待办已经收场（投影里没了）
  —— [8.2] 无游标的新客户端接上同一个会话，收到同一帧
  ✗ 重放没有把副作用再做一次（仍是 1） —— 实际 2，期望 1
  ✗ 也没有第二条回传 —— 实际 8，期望 7
=== 26 条通过，4 条失败 ===
```

修完 34 条全绿。两条突变都验过：

- **突变 1**（把求证结果无视，回到「收到帧就执行」）→ [8.2] 红（`实际 2，期望 1`）。
- **突变 2**（判据取反：「不在待办里才执行」）→ [8.3] 对照组红（`还欠着的活照样执行 ——
  实际 0，期望 2`），连 [8.1] 一起红。

**「必须重新 `import`」那条也实测过**，不是照抄结论：

```
原模块 vs ?reload=2 是同一个实例吗： false
两次同样的 ?reload=2 是同一个实例吗： true
```

`scripts/ts-resolve.mjs` 的钩子先试 `nextResolve(specifier)`，带扩展名 + query 的
`../src/tool-exec.ts?reload=2` 直接命中，**不用改脚手架**。所以「把认领集合提成模块级
单例」这种错修法骗不过 [8.2]。

### 比 issue 原文多做/少做的

- **多一条服务端测试**（取消那一路，见上）和**多两条前端断言**：[8.4] 是原文第 6 步
  （Gap：只往待办表里放、一帧都不喂，走「连上就拉」的入口），[8.5] 是原文没写但突变时
  发现必须有的——**帧和 `sweep` 共用同一份认领集合**，否则一条慢工具刚被帧触发、`sweep`
  就会在投影里又看见它（服务端那一刻确实还欠着）当场执行第二次。这是本方案自己引入的
  新缺口，不是原来的病。
- **前端 API 名字**：`fetchPendingTools` 而不是原文的 `fetchPendingWebTools`——端点返回
  的是**全部**远端等待（含 `desk:`），叫 `WebTools` 是句假话。按位置过滤放在 `tool-exec.ts`，
  跟它处理帧时同一条判据（`location === "Web"`）。
- **执行器多一个入口**：`createToolExecutor` 返回的东西从裸函数变成「可调用 + `.sweep()`」
  （`Object.assign`）。保持可调用是为了 `main.ts` 和既有 23 条断言一行不用改。
- **求证失败不执行**：GET 问不到（网络断/会话没了）时**退回认领、不做副作用**，等下一次
  `sweep` 重问（重连本来就会调它）。不知道就别下单。
- **`verify-tool-exec.ts` 顶破 300 行，就地拆了**：mock 端点（`POST /tool_result` +
  `GET /pending_tools` + 那张等待槽表）挪进 `scripts/tool-exec-mock-server.ts`（96 行），
  验收脚本只留断言（229 行）。红线 9。
- **既有 [2]–[7] 组补了一步**：现在每次派活先往 mock 的等待槽里登记（新增的 `dispatch()`
  辅助函数），跟服务端的真实顺序一致（`register` 在 `emit` 的上一行）。不补的话它们会
  因为「投影里没有」而全部不执行——这本身就是新判据真的在起作用的旁证。

### 协议面与既有消费者

- `SessionEvent` 变体数仍是 16，fixtures 没动，渲染层/`render/tool.ts` 一行没改。
- **Java 网关零改动**：`AgentProxyController` 的 `@RequestMapping("/agent/**")`
  （`examples/java-gateway/.../proxy/AgentProxyController.java:35`）按任意 method/path
  透传，新端点白拿；SSE 那条路没碰。
- 新端点对**死会话 410、休眠(dormant)/不存在 404**——跟 `GET /sessions/:id/agents` 同一条
  判据（`AppState::session_handle` 这一个函数），**不是** `GET /sessions/:id` 那条 073 的
  三态路。理由：等待槽是**运行时状态**（活在 actor 线程手上，actor 没了槽就没了），不是
  会话状态；对一个 dormant 会话答「空投影」等于告诉宿主「你不欠任何活」，而真相是「这个
  会话现在根本没在跑」，两者差着一次误判。

### 收工验证（全部前台跑完）

`cargo test -p agent-core -p agent-server -p agent-runtime` **790 通过 0 失败**；
`cargo test -p agent-server --features ts` 全绿（含 `generated_ts_matches_committed_snapshot`）；
`cargo clippy -p agent-runtime -p agent-server --all-targets -- -D warnings` 干净；
`scripts/check-invariants.sh --all` 通过；`pnpm --filter web typecheck` 干净；
`verify:tool-exec` 34/34、`verify:mcp` 46/46（既有的远端闭环
`web_tool_result_resumes_turn` / `web_tool_never_answered_times_out` 一并绿）。
突变全部还原，`grep -rn "MUTATION" crates/ packages/` 无命中。

### 留着没做（原文已备案，这里只确认没顺手做）

- **并发那一格**：同一个 chatid 同时挂多个执行端，两边求证都说「还欠着」→ 各执行一次。
  要 exactly-once 得把「认领」做成服务端状态变更（`POST /tool_claim` 走命令队列 + 应答），
  而 `Command` 现在是 fire-and-forget、没有应答通道。**已写进 `tool-exec.ts` 的文件头**，
  是宿主契约的一部分。
- `as_of` 帧 id 那个省一次往返的优化：不做（要引入一条时序规则）。响应体用
  `{ "pending": [...] }` 信封而不是裸数组，就是给这类元字段留的位置。
- 068 真机验收走网关那条路（demo 每次开页新建会话，碰不到这条 bug），不在本 issue。
