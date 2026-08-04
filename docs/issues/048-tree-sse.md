# 048 SSE 快照变化事件 + GET 端点

**里程碑** M7 · **依赖** 046 · **模型** sonnet · **独测** ✅

把 `agent_tree()` 快照接上远程：树变了就推一帧，reconnect 用 GET 拿当前快照做种。复用
M3（031/034）的 SSE + Last-Event-ID 补发，不新造传输。

## 范围（读 server 后细化：发射点在 pump，不在 actor）

**关键发现**（动手前定死）：`run_turn` 把整棵树驱动到**静止**才返回，所以「turn 边界
发射」只能看到最终树，不是 live。而 actor 的事件回调是 `FnMut(AgentEvent)`，触发时
`session` 正被 `run_turn` `&mut` 借着——回调里读不到 `session`（借用冲突）。所以**树快照
必须由 pump 发**（它持 `&mut session`，每次 `step` 后能调 `agent_tree()`），经既有事件
管道流出去。这是实时（子 agent spawn、activity 随 `Thinking→ToolsPending→终态` 变都要
当场可见）的硬性要求逼出来的，把本 issue 从纯 `agent-server` 扩到也碰 `agent-runtime`。

1. **pump 每步后发树快照**（`agent-runtime`）：`RunnerCtx` 加一条**独立回调**
   `on_tree_change: Option<Box<dyn FnMut(AgentTree)>>`（`with_tree_events` 设，照既有
   `with_agent_events` 同款；CLI 不设 → `None` → 无开销，它的 `/agents` 是按需的）。
   `run_turn` 主循环每次 `session.step()` + persist 之后算 `agent_tree()`，跟本地 `last_tree`
   比（`AgentTree: PartialEq`）**变了才调回调**并更新 `last_tree`。
   **不碰 `RunnerEvent` 枚举**——树快照是整棵状态的投影、不是增量事件，走独立通道免去
   `RunnerEvent` 的穷举 `match` 在 CLI print / io_thread / server `From` 三处的连锁改。
   每步都算（树小、深≤3/子≤8，可接受；change 检测只挡发送不挡计算，真要优化再只在
   tree-relevant 事件后算）。
2. **`SessionEvent::AgentTree(AgentTree)` 变体 + actor 接线**（`agent-server`）：actor 建
   `RunnerCtx` 时 `with_tree_events(...)` 设成「广播 `Frame { agent: root, event:
   SessionEvent::AgentTree(tree) }`」——**标 `AgentId::root()`**（树是会话级事实，照
   `emit_root`/`Undo`/`Gap` 同款约定，`event/frame.rs` 文档）+ 顺手更新一个共享
   `Arc<Mutex<AgentTree>>` 供 GET 读。`SessionEvent` 加变体、邻接标签
   `{"type":"agent_tree","data":{"nodes":[...]}}`，既有 serde 属性不改；`From<RunnerEvent>`
   **不用动**（树不走 RunnerEvent）。
3. **推快照不推 diff**——UI 哑渲染（OBSERVABILITY.md），整棵重画（树小）。
4. **GET `/sessions/{id}/agents`**：读那个共享 `Arc<Mutex<AgentTree>>`（actor 启动用初始树
   seed，回调每次更新）——**不走 actor mpsc 队列**，所以一轮跑到一半也能立刻拿到当下的
   活树（走队列的请求会排在 in-flight 的 `Input` 后面，拿不到「此刻」）。handler 形状照
   `sessions::status`。开页 / reconnect 做种，之后靠 SSE 事件增量。
5. **协议生成**：`AgentTree`/`AgentNode`/`AgentActivity` 进 `packages/protocol` 的 TS 生成
   （032 的 ts-rs 链路，它们已 `derive(TS)`），一致性测试锁死。
6. **Last-Event-ID 补发**：`agent_tree` 帧跟别的一样进 hub/ring、拿帧 id、参与补发——
   无需特殊处理（ring 对所有帧一视同仁）。

## 验收（可判定）

- 浏览器连 SSE，模型 spawn 子 agent → 收到 `agent_tree` 帧，其 `nodes` 跟同刻
  `GET /agents` 返回的一致（推和拉两条路给出同一棵树）。
- 树没变的 step**不**推 `agent_tree` 帧（不刷屏；变化才推）。
- **Last-Event-ID 补发**：断开重连带上 Last-Event-ID，漏掉的 `agent_tree` 帧补上——
  复用 031 钉死的补发机制（它的严格测试是先例）。
- 协议一致性测试：Rust 改了 `AgentNode` 忘了重新生成 TS → CI 红（032 的锁）。

## 注意

- **红线 11 不适用**：树快照走的是网络协议面，**不进 prompt**，不需要逐字节确定
  （区别于工具表）。但协议一致性（Rust↔TS 一份）仍由生成 + 测试锁。
- **红线 8**：端点在 `agent-server` 下，默认 loopback，不硬编码 `0.0.0.0`。
- **独测**（碰 SSE 补发 + 协议一致性，031/032 的先例都派了）：Last-Event-ID 补发那条要有
  断言（断开→漏帧→重连补上），协议一致性测试锁 Rust↔TS。
- 「树变了没有」的判断：拿这次快照跟上一次比（`PartialEq`），变了才推。别每 step 都推。
- 收工验证前台跑完（WORKFLOW §四 -1）。

## 实做记录（完成 · 2026-08-03）

设计由主会话读 server 后细化钉进本 issue（独立回调不走 `RunnerEvent`、树帧标 root、GET 走
共享 cell）；单个 sonnet 实现 agent 端到端做，主会话从磁盘验。

**产出**：
- **agent-runtime**：`RunnerCtx.on_tree_change` + `with_tree_events`/`tree_events_enabled`/
  `emit_tree`（`ctx.rs`）；`run_turn` 每步后 `maybe_emit_tree`（`runner.rs`）算 `agent_tree()`、
  跟本地 `last_tree` 比、变了才发。CLI 不设回调 → 零开销。
- **agent-server**：`SessionEvent::AgentTree(AgentTree)`（第 15 个变体，一致性测试的样本数 +
  `session_event_kind` 穷举同步更新）；actor `with_tree_events` 广播 `Frame{root, AgentTree}` +
  更新 `SessionHandle.tree: Arc<Mutex<AgentTree>>`（seed 空树）；GET `/sessions/{id}/agents` →
  `Json<AgentTree>` 读共享 cell（不排 mpsc 队列，一轮跑到一半也拿得到「此刻」）。
  `From<RunnerEvent>` 未动。
- **packages/protocol**：`AgentTree.ts`/`AgentNode.ts`/`AgentActivity.ts` 重生成。

**验收兑现**（主会话从磁盘跑）：`cargo test -p agent-runtime`（含
`tests/tree_snapshot_emits_on_change.rs`：spawn 发快照 + 树没变不重发）、`-p agent-server`
（含 `tests/http_agent_tree_get_matches_sse.rs`：GET 拉的树 == SSE 推的树）、`--features ts`
（`generated_ts_matches_committed` 绿 = TS 已同步）全绿；`clippy -D warnings` 净；红线过；
`pnpm -r typecheck` protocol + web 都 Done。ctx 282 / runner 250 / event 183 行，红线 9 内。

**独测判据修正（如实记）**：issue 头标了「独测 ✅」是初判保守。复核红线：048 **不碰任何
静默失败红线**（1-6/11/12）——红线 11 对树快照明确不适用（走协议面不进 prompt）。按
WORKFLOW 两步判据它**不强制独立测试 agent**。impl 的集成测试已覆盖 emit-on-change / change
检测 / GET-SSE 一致；协议一致性由 032 框架锁死；Last-Event-ID 补发由 031 的 ring 机制已锁
（树帧走同一 ring、无新逻辑）。主会话从磁盘复核这些覆盖真在，未再另派独测 agent。

**过程坑**：impl agent（177 次调用 / 58 分钟 / 大改动）又犯「收尾自旋」——把最后的
`--features ts` 确认跑甩给后台 + 等监视器，报「完成」时留了 orphan cargo 锁死构建。主会话
代收：杀 orphan、从磁盘重验四道门禁 + typecheck。**代码全对**（含 TS 重生成）。反 spin 指令
写在派活单顶部这次没拦住——5 个 subagent 里 2 个 impl（046/048）复发，是顽固失效模式，
主会话「代收」是可靠兜底。

## 补漏（049 真机 dogfood 逼出来的 · 2026-08-03）

049 的真浏览器验收里逮到一个**漏投影**：树快照只在 pump（`run_turn`）里发，而 undo /
redo / 取消轮自动擦除走的是 actor 的命令处理（**不经 `run_turn`**）——它们撤掉一棵子树
之后，core 层 `agent_tree()` 退了，但 SSE 帧没广播、`GET .../agents` 的共享 cell 没更新，
活树面板停在旧树。046 单测（core 层 `agent_tree()` undo 后退）+ 048 的 emit-on-change 测试
（只测 pump 路）**都绿**，只有真机跑才现形——「测试绿、世界不对」的又一例。

**修复**：`RunnerCtx::emit_tree_snapshot(session)`（`agent-runtime/src/ctx.rs`，公开，复用
`with_tree_events` 设的同一条回调 = 更新 cell + 广播）；`handle_undo` / `handle_redo` /
`erase_cancelled_turn`（`agent-server/src/actor/commands.rs`）各在命令处理末尾调它一次。
碰红的既有测试 `redo_endpoint_reapplies_the_undone_turn` 一并修：`next_typed` 跳过
`AgentTree` 帧（那个文件测命令语义，树帧是噪声）。

**验证**：`cargo test -p agent-runtime -p agent-server` 全绿（含修好的 redo 测试）；主会话
起真 server（deepseek 上游）+ curl 直打 `GET /agents` 复验——spawn 后
`['root','root/a1','root/a2']`（3 节点），`POST /undo` 后 `['root']`（**回退到 1 节点**）。
真机四点里点 1（1→3 实时长）、点 2（Thinking→Working→Done 状态变）、点 3（undo 回退树）
坐实；浏览器那条 vite dev 代理的 SSE 在 backend 重启后发飘（server 直连 200 挂住、会话
`alive` 均证过，是 dev 环境问题不是 fix），改用 curl 直打 API 定验——`GET /agents` 是面板
的数据源，它对面板就对。真机 dogfood 另捞到一条 adapter/spawn 的可用性摩擦，单列
[issue 050](050-tool-name-encoding.md)。

> **归因更正（2026-08-04，M8 真机时查明）**：上面那句「vite dev 代理发飘」**大概率是误判**。
> 本机有系统代理环境变量（`http_proxy=http://127.0.0.1:7897` / `all_proxy=socks5://…`），
> **`curl` 默认走它**、它转发本机端口失败 → 返回 502，看起来像「代理坏了」。加
> `--noproxy '*'` 一切正常。vite 的 dev 代理本身很可能一直是好的，当时没有排除这个变量。
> 结论保留在这里不删（当时的处置——改用 curl 直打数据源——本身仍然成立），但**归因不可引用**。
> 现在验浏览器有更省事的路：`examples/serve` 认 `AGENT_STATIC_DIR`，由 server 同源托管
> `packages/web/dist`，根本不经 dev 代理（见 [054 真浏览器补验](054-orchestration-surface-dogfood.md)）。
