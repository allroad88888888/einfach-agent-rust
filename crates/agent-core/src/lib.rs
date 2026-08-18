//! agent-core：agent 运行时的状态引擎与最小值类型集合。
//!
//! **红线 7**：这个 crate 不做 IO——没有网络、没有文件系统、没有子进程。
//! 整个 agent loop 要能在没有网络的情况下跑单元测试。
//!
//! 当前有 M1 需要的值类型（issue 021）、接缝词汇（025）、工具输出上限（004）、
//! 缓存兜底三层的判读（024）和 loop 的事件/effect 契约（001）。
//!
//! # 状态住在原子图里（026/027）
//!
//! [`command::Session`] 是会话形态：全部状态是 [`graph`] 上的 primitive atom，
//! 每次写入经 command 层留下一条 `Entry`，于是 undo / redo / 持久化 / 崩溃恢复 /
//! 审计回放是**同一份代码**的五个投影。转移表（002 定骨架、016 填停止条件、003
//! 填工具收敛）唯一住在 `command::transitions`，穷举表零 `unimplemented!`
//! （判断记录见各自 issue 文档的「实做记录」）。
//!
//! **M1 时代驱动一份平结构 `TurnState` 的 `engine::step` 已经退役**
//! （027：`docs/issues/027-cli-undo.md`）——`agent-runtime`/`agent-cli` 现在唯一
//! 驱动 `Session::step`。`engine` 模块留下的只是接缝词汇本身（`Event`/`Effect`/
//! `Notice`/`Epoch`/`TurnStatus`/`Failure`/`ToolSlot`/`SlotState`），026 的等价
//! 重写对照表记录了每一条 M1 行为现在由哪个 `session_*.rs` 测试保证。

pub mod cache;
pub mod command;
pub mod compaction;
pub mod engine;
pub mod graph;
pub mod ids;
pub mod limits;
pub mod observe;
pub mod seam;
pub mod value;

// 缓存兜底：**类型**提到根上，三个判读函数不提——`cache::reconcile(...)` 说得出
// 是在对什么账，裸的 `reconcile(...)` 说不出。
pub use cache::{
    DriftVerdict, GuardAlert, GuardLayer, GuardReport, PrefixIntent, ReconcileParams,
    ReconcileVerdict, TurnHit, WindowParams, WindowVerdict,
};
// 压缩的策略层（102 的第 2 档、108 的阶梯）：**类型和常量**提到根上，判读函数
// 不提——`compaction::tool_results_to_clear(...)` / `compaction::next_action(...)`
// 说得出是在清什么、在判哪一档，裸的名字说不出（同 `cache::reconcile` 的取舍）。
pub use compaction::{
    ClearParams, DEFAULT_PROTECT_RECENT_TURNS, DEFAULT_TRIGGER_PERCENT, LadderAction,
};
// 会话形态：类型提到根上，模块保留（`command::Session` 与 `agent_core::Session`
// 都通），跟 `engine` 一侧的惯例一致。
pub use command::{
    AgentEntry, AgentLimits, BarrierInfo, BlockedCause, BoundaryRejected, ChildConfig,
    ClearOutcome, DEFAULT_HISTORY_CAP, DEFAULT_MAX_AGENT_DEPTH, DEFAULT_MAX_CHILDREN,
    DeliverDenied, DespawnRefused, DespawnReport, EntryMeta, HookOutcome, MAX_NOTES,
    NOTE_KEY_CAP, NOTE_VALUE_CAP, NoteDenied, ReadDenied, Session, SpawnRefused,
    UndoReport, Undoability, known_label,
};
pub use engine::{Effect, Epoch, Event, Failure, Notice, SlotState, ToolSlot, TurnStatus};
pub use graph::{AtomKey, Slot, ToolCallSlot, Visibility};
pub use ids::{
    AGENT_PATH_SEP, AgentId, ExecutionProfileId, MessageId, SkillId, SummaryId, ToolCallId,
};
pub use limits::{DEFAULT_TOOL_OUTPUT_BYTES, truncate_tool_output, truncated_content_bytes};
pub use observe::{AgentActivity, AgentNode, AgentTree};
pub use seam::{
    Adjustment, ErrorClass, PrefixImage, RequestIntent, Segment, SegmentImage, SystemChunk,
};
pub use value::atom_value::AgentValue;
pub use value::host_skills::HostSkill;
// 收件箱（205，决策 35）：**类型提到根上，编解码不提**——`Deliver` 与 `InboxItem`
// 是运行时要构造和读取的东西，`to_value`/`from_value` 是槽位的内部形状。
pub use value::inbox::{Deliver, InboxItem};
pub use value::message::{ContentBlock, Message, Role};
// 压缩的发送侧坐标（099）：**类型和常量**提到根上，投影函数不提——
// `send_plan::project(...)` 说得出是在投什么，裸的 `project(...)` 说不出
// （同 `cache::reconcile` 的取舍）。
pub use value::send_plan::{BoundaryNotAdvancing, CLEARED_TOOL_RESULT, SendPlan};
pub use value::session::{SessionConfig, StopReason, TokenUsage};
pub use value::tool::{Location, Reversibility, ToolCallRequest, ToolSpec};
