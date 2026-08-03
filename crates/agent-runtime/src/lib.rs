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
//! | `ExecuteTool` | [`tool_exec::execute`] / [`dispatch`] | 按名字查 [`ToolTable`]，本地同步执行；`Irreversible` 的先 `mark_irreversible` 再执行。`srv:agent/spawn` 在分派处被截获，落到 `Session::spawn_child` |
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

mod dispatch;
mod guard;
mod io_thread;
mod provider_call;
mod runner;
mod skill;
mod spawn_tool;
mod subagent;
mod subtree;
mod tool_exec;
mod tool_table;

pub mod ctx;
pub mod event;
pub mod jsonl;
pub mod persist;

pub use ctx::RunnerCtx;
pub use event::{AgentEvent, RunnerEvent};
pub use jsonl::{Jsonl, SessionStoreError};
pub use persist::{PersistedMeta, RecoverError, SessionBackend, has_unresolved_tool_calls, open_backend, recover};
pub use runner::run_turn;
pub use skill::{SKILL_ACTIVATE, SKILL_DEACTIVATE, SkillLoadError, SkillRegistry};
pub use spawn_tool::{SPAWN_TOOL, spawn_spec};
pub use tool_table::ToolTable;
