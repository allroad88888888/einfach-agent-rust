//! 把 loop 接到真实 transport（issue 012），驱动的是 026 的 [`agent_core::Session`]
//! （027 换接——原先驱动 `TurnState` 的那一路已经退役，见 `docs/issues/027-cli-
//! undo.md`）。
//!
//! `Session::step` 是纯函数：喂一个 [`agent_core::Event`]，吐一批
//! [`agent_core::Effect`]，不做 IO。这个 crate 是把 `Effect` 真的执行掉、把
//! 执行结果翻译回 `Event` 再喂回去的那一圈——`run_turn`（[`runner::run_turn`]）
//! 循环到 [`agent_core::TurnStatus::is_terminal`] 为止（`TurnStatusChanged`
//! 通报是 loop 说「停」的唯一出口，见 agent-core engine/mod.rs 的文档）。
//! 027 额外接上两件事：每条命令之后经 [`persist`] 转发进 `SessionStore`
//! （011 的端口），以及派发 `ExecuteTool` 时按工具表的 `Reversibility` 调
//! `Session::mark_irreversible`（020 的屏障开闸）。
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
//! # 四个 effect，零 `unimplemented!`
//!
//! [`agent_core::Effect`] 现在只有四个变体（`SpawnChild`/`Compact`/`Persist`
//! 还没定），[`dispatch`] 的 `match` 穷举它们，四个全部真的执行：
//!
//! | effect | 谁执行 | 怎么执行 |
//! |---|---|---|
//! | `CallProvider` | [`provider_call::start`] | actor 线程取料 → `encode` → 发前 `check_drift` → 起 IO 线程跑 `post_stream`。**只起飞不落地**，落地由泵统一等（029 的并行就是这一刀） |
//! | `ExecuteTool` | [`tool_exec::execute`] / [`mcp_call::start`] / [`dispatch`] | 按名字查 [`ToolTable`]，本地同步执行；`Irreversible` 的先 `mark_irreversible` 再执行。`srv:agent/spawn`、`srv:agent/status`（051，纯读、当场回写）、skill 激活在分派处被截获；`mcp:` 前缀且工具表声明的走**异步第四路**（`mcp_call`，不进 `ToolExecutor`），epoch 回写前过闸（红线 6，043） |
//! | `CancelInFlight` | [`dispatch`] | 置共享的取消标志 + 斩断队列里还没喂进去的待办 |
//! | `Emit(Notice)` | [`dispatch`] | 带上「出自谁的 `step`」转给 [`RunnerCtx`] 的事件回调 |
//!
//! # 子 agent（029）
//!
//! `run_turn` 驱动的是**整棵树**：`srv:agent/spawn` 长出子 agent，子 agent 的
//! provider 调用各自一个 IO 线程真的并行，回写全部串行过泵；子 agent 落终态时
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

mod child_outcome;
mod collect_tool;
mod ctx_remote_tools;
mod deadline;
mod dispatch;
mod guard;
mod io_thread;
mod mcp_call;
mod orphan;
mod provider_call;
mod remote_tool;
mod remote_tool_claim;
mod remote_tool_digest;
mod remote_tool_protocol;
mod remote_tool_receipt;
mod remote_tool_status;
mod remote_tool_submission;
mod reply;
mod runner;
mod skill;
mod spawn_tool;
mod status_tool;
mod subagent;
mod subtree;
mod tool_exec;
mod tool_name;
mod tool_table;

pub mod ctx;
pub mod event;
pub mod jsonl;
pub mod persist;

pub use agent_mcp::McpRegistry;
pub use collect_tool::{COLLECT_TOOL, collect_spec};
pub use ctx::RunnerCtx;
/// 072：远端等待槽的只读投影形状。`ctx_remote_tools` 本身是私有模块（等待槽只能
/// 由 actor 线程改），但**投影是要跨层出去的**——`agent-server` 拿它填
/// `GET /sessions/{id}/pending_tools` 的响应体。
pub use ctx_remote_tools::RemoteToolWaiting;
pub use deadline::sweep_remote_tool_deadlines;
pub use event::{AgentEvent, OrphanFate, RunnerEvent};
pub use jsonl::{Jsonl, SessionStoreError};
pub use persist::{
    PersistedMeta, RecoverError, SessionBackend, has_unresolved_tool_calls, open_backend, recover,
};
pub use remote_tool::{
    RemoteToolOutput, RemoteToolResultError, cancel_pending_remote_tools, resolve_remote_tool,
};
pub use remote_tool_claim::claim_remote_tool;
pub use remote_tool_protocol::{
    RemoteToolActive, RemoteToolActiveState, RemoteToolClaimDecision, RemoteToolClaimGrant,
    RemoteToolClaimRequest, RemoteToolFailure, RemoteToolReceipt, RemoteToolStatusSnapshot,
    RemoteToolSubmitDecision, RemoteToolSubmitOutcome, RemoteToolSubmitRequest,
    RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};
pub use remote_tool_receipt::REMOTE_TOOL_RECEIPT_CAP;
pub use remote_tool_submission::submit_remote_tool_result;
pub use runner::{run_turn, run_turn_with_images};
pub use skill::{SKILL_ACTIVATE, SKILL_DEACTIVATE, SkillLoadError, SkillRegistry};
pub use spawn_tool::{SPAWN_TOOL, spawn_spec};
pub use status_tool::{STATUS_TOOL, status_spec};
pub use tool_table::ToolTable;
