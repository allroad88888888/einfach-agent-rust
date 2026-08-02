//! `Session` 词汇里跟「一轮走到哪了」相关的那几个类型：状态、失败原因、工具槽。
//!
//! **027 之前这里还有 `TurnState`**（一轮对话的平结构状态，`engine::step` 驱动
//! 它）——026 把状态原生迁进了原子图（`command::Session` + `graph::Slot`），
//! 027 把 runner/CLI 换接到 `Session` 之后 `TurnState` 退役。这个文件剩下的
//! 四个类型（`TurnStatus`/`Failure`/`ToolSlot`/`SlotState`）不是 `TurnState`
//! 的附属品，是它们自己：`Slot::default_value()`（`graph/slot.rs`）用
//! `TurnStatus::Idle`/`SlotState` 给对应槽位落初值，`Session` 的读口
//! （`command/read.rs`）原样透传这四个类型给宿主——它们本来就是**接缝词汇**
//! （001 的裁决），只是曾经恰好也被 `TurnState` 引用过。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::ToolCallId;
use crate::seam::ErrorClass;

/// `Slot::MaxTurns` 的默认上限（016）。026 起是 `pub`：`graph::Slot::default_value()`
/// 要用同一个数给 `Slot::MaxTurns` 落初值——这是 M1 引擎与 `Session` 曾经共用的
/// 一个数，027 退役 M1 引擎之后，它是 `Session` 唯一的读者。
pub const DEFAULT_MAX_TURNS: u32 = 32;

/// `Slot::MaxRetries` 的默认上限（016）。026 起是 `pub`，理由同 [`DEFAULT_MAX_TURNS`]。
pub const DEFAULT_MAX_RETRIES: u32 = 2;

/// 一轮走到哪了。转移 `Idle → Thinking → ToolsPending → Done | Failed` 的语义现在
/// 唯一住在 `command::transitions`（026/027），这里只定形状。
///
/// 终态带着**为什么终**，而不是只给一个光秃秃的 `Done`：016 的验收原文是「调用方能
/// 区分『答完了』和『被截断了』」，014 每轮要把这个判读打出来。区分放在状态里而不是
/// 另发一条通报，是因为它是**状态**——事后任何时候问「这轮怎么结束的」都该答得出，
/// 而通报是一次性的，谁没接住就没了。
/// 032：经 `Notice::TurnStatusChanged` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum TurnStatus {
    /// 没有在飞的东西，等用户输入。
    Idle,
    /// provider 调用在飞。
    Thinking,
    /// 至少一个工具槽还是 [`SlotState::Pending`]。
    ToolsPending,
    /// 这轮结束了。
    Done {
        /// `true` = 撞了轮数闸被截断的（016），模型还想接着干。
        /// `false` = 模型自己说完了（`StopReason::EndTurn`）。
        truncated: bool,
    },
    /// 这轮没能走完。
    Failed(Failure),
}

impl TurnStatus {
    /// 终态判定。runner（012）驱动到这里为止，CLI（014）据此换行打摘要。
    ///
    /// 写成方法而不是让调用方各自 `matches!`：加终态变体时（比如 M2 的
    /// 「撞上 `Irreversible` 停下来问」）只有这一处要改。
    pub fn is_terminal(&self) -> bool {
        matches!(self, TurnStatus::Done { .. } | TurnStatus::Failed(_))
    }
}

/// 一轮失败的原因。**只有这两类**——工具失败不在里面，那是 003 的裁决：
/// 3 个工具 2 成功 1 失败，失败当 `tool_result`（`is_error: true`）喂回模型，
/// 由它决定要不要紧，loop 不替它中止。
/// 032：经 `TurnStatus::Failed` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Failure {
    /// 用户取消（016）。
    Cancelled,
    /// provider 报错，且按 [`ErrorClass`] 分流之后判定不该重试。
    /// **core 不自己看状态码**（红线 12）：分类是 adapter `classify` 的产物。
    Provider(ErrorClass),
}

/// 一个在飞（或已回来）的工具调用槽。
///
/// 只存 core 真有的东西：模型给的名字和输入（红线 3 的精神：纯数据不是句柄）。
/// `Location`/`Reversibility` 的**发起时快照**由持有注册表的宿主在路由时构造、
/// M2 的 command 层记录（009 的 `Entry`）——core 不编造自己没有的数据，
/// 理由见 [`super::Effect::ExecuteTool`]。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ToolSlot {
    pub call_id: ToolCallId,
    pub tool: Arc<str>,
    pub input: Arc<serde_json::Value>,
    pub state: SlotState,
}

/// 槽位状态。**`Pending` 是一个显式变体，不是 `Option::None`**——收敛判断要扫的
/// 就是它，写成 `Option` 会让「还没回来」和「回来了但没内容」在读代码时糊成一团。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum SlotState {
    /// 发出去了，还没回来。
    Pending,
    /// 回来了。`is_error` 直接对应 `ContentBlock::ToolResult::is_error`——
    /// 失败也是一条结果，会进下一轮 prompt（003）。
    Finished { content: Arc<str>, is_error: bool },
}
