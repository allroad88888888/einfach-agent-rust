//! 落盘的逻辑键 [`AtomKey`]（红线 4）、一次工具调用自己的槽位 [`ToolCallSlot`]、
//! derived atom 的键 [`DerivedKey`]。
//!
//! 154 从 [`slot`](super::slot) 拆出来（那个文件加 `Slot::HostPrefix` 顶破了 300
//! 行）：[`slot`](super::slot) 回答「一个（agent 的）槽位怎么称呼」，这里回答
//! 「落盘的键长什么样」——`AtomKey` 是「哪个 agent / 哪次工具调用」+「[`Slot`]」，
//! 装的是 `Slot`、不是反过来，天然是两件事。
//!
//! `AtomKey` 的**两个变体一个不少**，即使 M2 只构造 `Agent` 那一支：它是落盘键的
//! 类型，改它的形状等于让所有旧日志/快照解不出来。`Slot` 可以往里加（旧快照缺键
//! 用默认值），`AtomKey` 的变体集合不能事后改——两者的稳定性要求不是一个量级。

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ToolCallId};
use crate::value::awaiting::AwaitUntil;

use super::slot::Slot;

/// 一次工具调用自己的槽位。
///
/// M2 只有 `Result` 一个：`Request`（发起当时的 `Location` / `Reversibility` 快照，
/// STATE-MODEL §「落盘的键必须是 AtomKey」）要等**持有工具表的宿主**来记——core
/// 没有工具表，现造一份占位快照是编造（002 合并时的裁决：假的 `Irreversible`
/// 会让 undo 白拦一次 `fs/read`，正是静默错值）。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum ToolCallSlot {
    /// 在飞时持 [`AgentValue::Pending`]，回来后持内容。
    Result,
}

/// 落盘的逻辑键。`Snapshot` 与 `Entry.changes` 用它，`AtomId` 只在进程内有效。
///
/// **只有两个变体**。没有 `Skill(SkillId)`——skill 的内容在 store 外的 registry 里，
/// store 里只有「哪些被激活」，那是某个 `Agent(_, _)` 槽位（STATE-MODEL）。
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub enum AtomKey {
    Agent(AgentId, Slot),
    ToolCall(AgentId, ToolCallId, ToolCallSlot),
}

impl AtomKey {
    /// 这个键属于哪个 agent。`undo` 不看它（一条扁平日志按时间排序），
    /// 逐出与 UI 时间线看它。
    pub fn agent(&self) -> &AgentId {
        match self {
            AtomKey::Agent(a, _) | AtomKey::ToolCall(a, _, _) => a,
        }
    }
}

/// derived atom 的键。**刻意不 derive serde**：derived 不进日志也不进快照
/// （它们全部可重算，这正是「完整状态 = 所有 primitive」成立的原因），给它一个
/// `Serialize` 就是给「把算出来的值也存一份」开了口子。
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum DerivedKey {
    /// 「本 agent 的工具槽全都不是 `Pending` 了吗」。003 预言的那个 derived。
    ToolsConverged(AgentId),
    /// 「`target` 到达 `until` 了吗」——`srv:agent/await` 的那个（212，决策 35 §一）。
    ///
    /// **这是全系统第二种 derived，也是第一条跨 agent 的边**。在它之前，
    /// `args.get` 在生产代码里只有 `build` 一处、读的还是**自己 agent** 的
    /// `ToolSlots`；跨 agent 的读走的是命令层的非追踪读（`cross_read`），
    /// 一条边都不建。
    ///
    /// **键里没有等待方**：值只取决于「谁、等到什么」。两个 agent 等同一个目标、
    /// 同一个条件时共用一个 derived——那正是想要的（一次重算，两个人都看得到）。
    ///
    /// 无环的判据落在它身上：read fn 里的 `args.get` 只能拿 `Slot` 去构
    /// `AtomKey::Agent`，而那永远落在 source family 上，**primitive 没有出边**
    /// ——所以这条边是一条长度 1 的悬边，绕不回来（红线 10 的新形态）。
    AwaitReached {
        target: AgentId,
        until: AwaitUntil,
    },
}
