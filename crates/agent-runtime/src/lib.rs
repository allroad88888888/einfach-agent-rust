//! 把 loop 接到真实 transport（issue 012），驱动的是 026 的 [`agent_core::Session`]
//! （027 换接——原先驱动 `TurnState` 的那一路已经退役，见 `docs/issues/027-cli-
//! undo.md`）。
//!
//! `Session::step` 是纯函数：喂一个 [`agent_core::Event`]，吐一批
//! [`agent_core::Effect`]，不做 IO。这个 crate 是把 `Effect` 真的执行掉、把
//! 执行结果翻译回 `Event` 再喂回去的那一圈——`run_turn`（[`runner_entry::run_turn`]）
//! 循环到 [`agent_core::TurnStatus::is_terminal`] 为止（`TurnStatusChanged`
//! 通报是 loop 说「停」的唯一出口，见 agent-core engine/mod.rs 的文档）。
//! 027 额外接上两件事：每条命令之后经 [`persist`] 转发进 `SessionStore`
//! （011 的端口），以及派发 `ExecuteTool` 时按工具表的 `Reversibility` 调
//! `Session::mark_no_undo`（020 的屏障开闸）。
//!
//! # 为什么不进 `agent-core`，也不进 `agent-transport`
//!
//! 红线 7：`agent-core` 不做 IO，loop 因此能零网络跑穷举测试——这个 crate 全是
//! IO（HTTP、文件系统、线程、时钟），放进 `agent-core` 会直接违反那条红线。
//! 不放 `agent-transport`：那个 crate 只管「怎么发一次 HTTP 请求」，不认识
//! `agent-core` 的事件/effect 词汇，也不认识 `agent-tools` 的 executor——
//! 塞进去会让一个只做传输的 crate 长出编排职责，一个 crate 一件事。
//!
//! # 为什么独立成库，不直接写进 `agent-cli`
//!
//! M3 的 server 会复用同一个 `run_turn`：CLI 和 server 只是 runner 的两种
//! 宿主，差别只在 [`RunnerEvent`] 回调把通报送去哪（stdout 还是 SSE）。写进
//! `agent-cli` 会让 M3 落地时把这坨编排逻辑从一个 bin crate 里整体搬出来，
//! 现在直接独立成库不亏。
//!
//! # 五个 effect，零 `unimplemented!`
//!
//! [`agent_core::Effect`] 现在有五个变体（`SpawnChild`/`Persist` 还没定；
//! `Compact` 是 105 在 M12 加的第五个），[`dispatch`] 的 `match` 穷举它们，
//! 五个全部真的执行：
//!
//! | effect | 谁执行 | 怎么执行 |
//! |---|---|---|
//! | `CallProvider` | [`provider_call::start`] | 泵所在线程取料 → `encode` → 发前 `check_drift` → 把一个跑 `post_stream` 的 future 交给泵（117 之前是起一条 IO 线程）。**只起飞不落地**，落地由泵统一等（029 的并行就是这一刀） |
//! | `ExecuteTool` | [`tool_exec::execute`] / [`mcp_call::start`] / [`dispatch`] | 按名字查 [`ToolTable`]，本地同步执行；`Irreversible` 的先 `mark_no_undo` 再执行。`srv:agent/spawn`、`srv:agent/status`（051，纯读、当场回写）、skill 激活在分派处被截获；`mcp:` 前缀且工具表声明的走**异步第四路**（`mcp_call`，不进 `ToolExecutor`），epoch 回写前过闸（红线 6，043） |
//! | `Compact` | [`compact_spawn::intercept`] | 106：spawn 一个窄范围子 agent（`ChildConfig::execution_profile` 来自 [`ctx::RunnerCtx::with_compaction_execution_profile`]，`agent-core` 没有为摘要新增任何 provider 分支），把 `[0, upto)` 那段历史渲染成它的第一条 user 消息；子落终态由 [`compact_slot::CompactSlots`] 收割成 `Event::CompactDone`/`CompactFailed`。spawn 被拒或子agent失败都是**正常事件**（`CompactFailed`，原样带回同一个 `epoch`）——压缩这一次作废、边界不动、下一轮照常跑 |
//! | `CancelInFlight` | [`dispatch`] | 置共享的取消标志 + 斩断队列里还没喂进去的待办 |
//! | `Emit(Notice)` | [`dispatch`] | 带上「出自谁的 `step`」转给 [`RunnerCtx`] 的事件回调 |
//!
//! # 子 agent（029）
//!
//! `run_turn` 驱动的是**整棵树**：`srv:agent/spawn` 长出子 agent，子 agent 的
//! provider 调用各自一个 IO future、在同一个事件循环上真的并行（117 之前是各自
//! 一条 IO 线程，见 [`io_bus`]），回写全部串行过泵；子 agent 落终态时
//! 它的最后一段文本作为 `tool_result` 回到父那个 spawn 槽（决策 20，不需要
//! `ChildFinished` 事件也不需要汇聚 derived）。整轮共用一个 `turn_id`（root 铸，
//! 决策 5），所以 `/undo` 一轮连带整棵子树。
//!
//! # 后台子 agent（052）
//!
//! `spawn(background=true)` 把上面那条路拆成两半：建子之后**立刻**回一条只装
//! `agent_id` 的 `tool_result` 收敛父的槽（父不被挡，同一 turn 里接着干活），子
//! 记进 [`subtree`] 的 detached 名单；它落终态时结果进「已完成未领取」的 stash
//! 而**不回写父**（那槽早收敛了，再回写就是幽灵结果）。子 agent 依旧**不跨
//! turn**：root 落终态时还没人领的活后台子由 [`orphan`] 定点 `despawn_child`
//! 拆掉（不走会话级取消——那会把答成功的一轮判成 `Failed(Cancelled)`）。
//!
//! # 领结果：`srv:agent/collect`（053）
//!
//! 后台那半边的另一头。`collect(id)` 要么当场端走 stash 里那份结果（领取即消费），
//! 要么给还在跑的子**补一个槽**——从补上那一刻起它跟前台 spawn 出来的子逐字同一条
//! 收割路（父 `ToolsPending`、泵驱动子、终态回写）。于是「前台 spawn ≡ spawn(bg) +
//! 紧跟 collect」在代码上是真的：两者共用 [`subtree`] 的同一张槽位表，
//! 差别只在模型什么时候把那一笔记上。绑了 collect 的子**不是孤儿**，[`orphan`]
//! 的轮末清算认这条。
//!
//! # 自动压缩的阶梯（108）
//!
//! `Effect::Compact` 的**产出方**在这个 crate，不在 core：[`compact_ladder`] 在
//! 每一轮 root 落 `Done` 时问一次 `agent_core::compaction::next_action`（纯函数，
//! 红线 1），第 2 档当场走 `Session::clear_tool_results` 命令，第 3 档产出一条
//! `Effect::Compact` 并进这一步的 effect 批。**一轮只判一次**——「第 2 档清完还
//! 不够」要靠下一轮实测，同一轮里再判一次就退化成推断（红线 12）。
//!
//! 摘要回来之后的回写是两步，中间隔着红线 6 的闸：`Event::CompactDone` 先过
//! `Session::step`，**回执里出现 `Notice::CompactionSummaryReceived` 才**调
//! `Session::apply_summary`（107 留下的硬契约，判据显式住在
//! [`compact_writeback::passed_epoch_gate`]，`upto` 由 [`compact_slot`] 记着）。
//! 摘要子 agent 收割完当场 `despawn_child`——它是一次性工人，不回收的话
//! `max_children` 默认 8，长会话压 8 次之后自动压缩永久失效。
//!

mod auto_turn;
mod block_on;
mod builtin_intercepts;
mod child_outcome;
mod child_slot;
mod collect_tool;
mod compact_ladder;
mod compact_slot;
mod compact_spawn;
mod compact_writeback;
#[cfg(test)]
mod compaction_visibility_tests;
mod ctx_remote_tools;
mod deadline;
mod dispatch;
mod execution_binding;
mod extension_pack;
mod guard;
mod heartbeat;
mod host_declaration;
mod intercept_registry;
mod io_bus;
mod io_stream;
mod io_task;
mod mcp_call;
mod notes_render;
mod notes_tool;
mod orphan;
mod provider_attempt;
mod provider_call;
mod provider_call_finish;
mod provider_message;
mod remote_tool;
mod remote_tool_claim;
mod remote_tool_digest;
mod remote_tool_protocol;
mod remote_tool_receipt;
mod remote_tool_status;
mod remote_tool_submission;
mod reply;
mod runner;
mod runner_entry;
mod self_render;
mod self_tool;
mod send_tool;
mod session_start;
mod session_tool_ext;
mod skill;
mod spawn_request;
mod spawn_tool;
mod status_render;
mod status_tool;
mod subagent;
mod subtree;
mod tool_exec;
mod tool_name;
mod tool_table;
mod transient_source_completion;
#[cfg(test)]
mod transient_source_completion_tests;
mod transient_source_failure;
mod transient_source_ingress;
mod transient_source_policy;
mod transient_source_prompt;
mod transient_source_recovery;
#[cfg(test)]
mod transient_source_tests;
mod transient_source_vault;
mod turn_end;
mod undo_hook;
mod undo_promise;
mod unread_inbox;

pub mod ctx;
pub mod event;
pub mod jsonl;
pub mod persist;
/// 201：三个宿主唯一该调的撤销入口——**带着钩子表**调 `Session` 的 `*_with`
/// （决策 199 §三）。`pub mod` 而不是把三个函数提到根上：`agent_runtime::undo::
/// undo_turn(session, ctx)` 在调用点一眼看得出「这是带钩子的那一档」，而根上一个
/// 光秃秃的 `undo_turn` 跟 `Session::undo_turn` 长得一模一样，正是最容易调错的形状。
pub mod undo;

pub use agent_mcp::McpRegistry;
/// 211：自驱动的轮次——留言自己就能把下一轮启动（决策 35 §二）。
/// **恢复路径该调的是 `report_recovered_mail`，不是 `run_auto_turns*`**，
/// 那条选择刻意长在调用点上，不藏在一个 `if recovered` 里。
pub use auto_turn::{
    AutoTurnStep, pending_next_turn_mail, report_recovered_mail, run_auto_turns_async,
    try_one_auto_turn_async,
};
#[cfg(not(target_arch = "wasm32"))]
pub use auto_turn::run_auto_turns;
pub use block_on::block_on;
pub use collect_tool::{COLLECT_TOOL, collect_spec};
pub use notes_tool::{NOTES_SET_TOOL, NOTES_TOOL, notes_set_spec, notes_spec};
pub use self_tool::{SELF_TOOL, self_spec};
pub use send_tool::{SEND_TOOL, send_spec};
pub use ctx::RunnerCtx;
/// 072：远端等待槽的只读投影形状。`ctx_remote_tools` 本身是私有模块（等待槽只能
/// 由 actor 线程改），但**投影是要跨层出去的**——`agent-server` 拿它填
/// `GET /sessions/{id}/pending_tools` 的响应体。
pub use ctx_remote_tools::RemoteToolWaiting;
/// 123：一条等待槽还剩多久到点。就地执行宿主工具的形态（浏览器）拿它把那次
/// `await` 变成可打断的等待，跟 [`sweep_remote_tool_deadlines_async`] 是同一份
/// 截止线的两个用法——一个问「还剩多久」，一个做「到点了怎么收」。
pub use deadline::remote_tool_deadline_in;
#[cfg(not(target_arch = "wasm32"))]
pub use deadline::sweep_remote_tool_deadlines;
pub use deadline::sweep_remote_tool_deadlines_async;
pub use event::{AgentEvent, AutoTurnHold, OrphanFate, RunnerEvent};
pub use execution_binding::ExecutionBinding;
/// 148：一个 Rust 扩展的交付物（决策 29）。宿主 `with_extension` 一次吃一包，
/// 装配是两阶段的——ctx 半边那个必须被消费的中间产物是
/// [`PendingInterceptors`]，接缝文档 `docs/EXTENSIONS.md`。
pub use extension_pack::ExtensionPack;
/// 122：一份宿主工具声明 JSON → [`ToolTable::with_host_tools`] 要的料。纯函数，
/// 给**没有 HTTP 那一层**的宿主用（浏览器宿主 `agent-wasm` 是第一个）；server
/// 形态走的仍是自己那份绑着请求体与 `ts-rs` 的 `http/capabilities`，两份的分界与
/// 漂移风险写在 [`host_declaration`] 模块文档里。
pub use host_declaration::{HostDeclarationError, host_tools_from_declaration};
/// 146：`RunnerCtx::register_session_tool` 收的公开层签名——`intercept_registry`
/// 本身是私有模块（注册表只能在装配期改），但这个类型要跨层出去：扩展/独测
/// 拿它构造要注册的闭包。
pub use intercept_registry::SessionToolFn;
pub use jsonl::{Jsonl, SessionStoreError};
pub use persist::{
    PersistedMeta, RecoverError, SessionBackend, has_unresolved_tool_calls, open_backend, recover,
};
pub use remote_tool::{
    RemoteToolOutput, RemoteToolResultError, ResolveRemoteToolError,
    cancel_pending_remote_tools_async, resolve_remote_tool_async,
};
#[cfg(not(target_arch = "wasm32"))]
pub use remote_tool::{cancel_pending_remote_tools, resolve_remote_tool};
pub use remote_tool_claim::claim_remote_tool;
pub use remote_tool_protocol::{
    RemoteToolActive, RemoteToolActiveState, RemoteToolClaimDecision, RemoteToolClaimGrant,
    RemoteToolClaimRequest, RemoteToolFailure, RemoteToolReceipt, RemoteToolStatusSnapshot,
    RemoteToolSubmitDecision, RemoteToolSubmitOutcome, RemoteToolSubmitRequest,
    RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};
pub use remote_tool_receipt::REMOTE_TOOL_RECEIPT_CAP;
#[cfg(not(target_arch = "wasm32"))]
pub use remote_tool_submission::submit_remote_tool_result;
pub use remote_tool_submission::submit_remote_tool_result_async;
/// native 的公开入口是同步的（`run_turn`），wasm 上只有 `run_turn_async`——
/// 成对的理由与那处 `cfg` 的取舍见 [`runner`] 模块文档「但公开入口在 native 上
/// 仍然是同步的」。远端工具的四个入口（`resolve_remote_tool`、
/// `cancel_pending_remote_tools`、`submit_remote_tool_result`、
/// `sweep_remote_tool_deadlines`）同款成对。
#[cfg(not(target_arch = "wasm32"))]
pub use runner_entry::run_turn;
pub use runner_entry::run_turn_async;
pub use session_start::{SessionStartError, run_session_start};
pub use skill::{SkillLoadError, SkillRegistry};
pub use spawn_request::{SPAWN_TOOL, spawn_spec};
pub use status_tool::{STATUS_TOOL, status_spec};
pub use tool_table::extension::PendingInterceptors;
pub use tool_table::{CallTiming, TimedRun, TimedTool, ToolTable};
pub use transient_source_failure::TransientSourceFailure;
/// 124：跨 crate 判定一个工具名是不是 transient-source（`web:source/`），
/// 不带前缀常量本身——见 [`transient_source_policy::is_transient_source`] 文档。
pub use transient_source_policy::is_transient_source;
pub use transient_source_recovery::recovered_transient_source_needs_fail_close;
/// 201：截获式工具执行完交代的那件事——「这次调用在外部世界留下了什么」
/// （决策 199 §一）。扩展作者写 [`SessionToolFn`] 时要构造它，所以是公开类型；
/// 三态与 `agent_core::Undoability` 一一对应，翻译在 [`session_tool_ext`] 做。
pub use undo_hook::{Aftermath, UndoFn};

/// 202：这次调用声明的可逆性是不是一个**本仓兑现不了的承诺**（决策 199 §七
/// 「承诺挡，事实不挡」）。跨 crate 公开是因为**显示层要跟行为共用同一个判据**：
/// `agent-cli` 打工具卡片时拿它决定要不要在 `reversibility` 后面补一句
/// 「本仓不代为补偿」。三格取舍与「为什么不能各写一遍」见 [`undo_promise`] 模块文档。
pub use undo_promise::is_unkeepable_promise;
