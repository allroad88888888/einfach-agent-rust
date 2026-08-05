//! 一条 `Entry` 的元数据，以及 agent 侧那三个日志类型的别名。
//!
//! 009 把 `Entry` 泛型化成 `Entry<K, V, M>`：`turn_id` / `epoch` / `label` 全是 agent
//! 词汇，而 `History` 住在 `agent-store`，那个 crate 不许 import `agent-core`
//! （ARCHITECTURE §包结构）。整组字段因此成为泛型 `M`——[`EntryMeta`] 就是 026
//! 把它填上的那一份。

use serde::Serialize;

use crate::engine::epoch::Epoch;
use crate::graph::AtomKey;
use crate::value::atom_value::AgentValue;

/// 一个 undo 步的元数据。
///
/// **刻意不 derive `Deserialize`**：`label` 是 `&'static str`，而从运行时字节反
/// 序列化借不出 `'static`。落盘的 schema 归 011（`SessionStore`），它那一侧的
/// `label` 是 `String`——两者形状不同是对的：进程内的标签取值是有限的编译期常量集，
/// 落盘的标签是历史数据，允许出现这个版本不认识的取值。
#[derive(Clone, PartialEq, Eq, Debug, Serialize)]
pub struct EntryMeta {
    /// 两层粒度靠它分组，**由 root agent 分配**：子 agent 的 entry 继承所在 root
    /// turn 的 `turn_id`，不产生新的 turn 边界。于是 `undo_turn` 一次退回一整个
    /// root turn，连带那一轮里所有子 agent 的工作（STATE-MODEL §「Command log」）。
    pub turn_id: u64,
    /// 写这一条时的世代（红线 6 的凭证）。undo 时 bump 的是 session 的 epoch，
    /// **不回滚**——它只增不减，这条记的是「这一步发生在哪一代」，用于审计与
    /// 崩溃恢复时把 epoch 接着往下发（恢复后取日志里的最大值 + 1）。
    pub epoch: Epoch,
    /// 这一步是什么（`"user_input"` / `"provider_done"` / …）。进 UI 时间线与审计。
    pub label: &'static str,
    /// **不可越过的屏障**（020 的落点）：这一步记录了一次 `Irreversible` 工具调用的
    /// 结果，undo 走到这里要停下来问用户（`UndoOutcome::Blocked`）。
    ///
    /// 谁来置真：宿主在派发工具时调 [`Session::mark_irreversible`](super::Session::mark_irreversible)
    /// ——**core 没有工具表**，`Reversibility` 是工具描述符上的元数据，core 现造一个
    /// 等于编造（002 合并时的裁决）。
    pub barrier: bool,
}

/// 一处源状态变更。键是逻辑键（红线 4）。
pub type AgentChange = agent_store::Change<AtomKey, AgentValue>;

/// 一个 undo 步 = 一次 `store.batch` 里的全部变更。
pub type AgentEntry = agent_store::Entry<AtomKey, AgentValue, EntryMeta>;

/// agent 侧的 command log。
pub type AgentHistory = agent_store::History<AtomKey, AgentValue, EntryMeta>;

/// turn 粒度的判据：比 `turn_id`。喂给 `History::undo_turn` / `redo_turn`。
pub(crate) fn same_turn(a: &EntryMeta, b: &EntryMeta) -> bool {
    a.turn_id == b.turn_id
}

/// 屏障谓词：`barrier` 为真的条目不可越过。喂给 `History::undo_turn`。
pub(crate) fn is_barrier(meta: &EntryMeta) -> bool {
    meta.barrier
}

/// 全部合法的 `label` 取值——[`transitions::label_of`](super::transitions) 的七个 +
/// `Session` 会话级命令的四个（`begin_turn` / `set_max_turns` / `set_max_retries` /
/// `clear_prev_prefix`）+ 028 的两条树形命令（`spawn_child` / `despawn_child`）+ 039
/// 的两条 skill 命令（`activate_skill` / `deactivate_skill`）+ 073 的宿主注入声明
/// （`declare_host_tools`）。
/// 这是一个**封闭的、有限的编译期常量集**（`EntryMeta.label` 的文档注释）。
///
/// **加一条就是一次协议变更**：用新代码写出来的会话文件，旧二进制打开时会在这里
/// 认不出这个标签，`recover` 硬失败而不是编一个假的凑合用（[`known_label`]）。
const KNOWN_LABELS: &[&str] = &[
    "user_input",
    "provider_done",
    "provider_failed",
    "tool_result",
    "tool_failed",
    "timeout",
    "cancel",
    "begin_turn",
    "set_max_turns",
    "set_max_retries",
    "clear_prev_prefix",
    "spawn_child",
    "despawn_child",
    "activate_skill",
    "deactivate_skill",
    "declare_host_tools",
    "declare_host_skills",
    "disable_builtins",
];

/// 把落盘的 label 字符串映射回编译期常量 `&'static str`。
///
/// **为什么需要这个函数**：`EntryMeta` 刻意不 derive `Deserialize`
/// （`label: &'static str` 借不出 `'static`，见类型文档），所以持久化层（027 的
/// `agent-runtime`）落盘时把 `label` 存成 `String`，载入时要把它变回来。labels 是
/// 一个有限的封闭集合，这个函数就是那张对照表——**不认识的字符串返回 `None`**，
/// 调用方据此拒绝加载（这版代码不认识的历史标签，不能编一个假的凑合用，那是
/// 静默错值）。
pub fn known_label(label: &str) -> Option<&'static str> {
    KNOWN_LABELS.iter().find(|&&known| known == label).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_label_maps_back_to_itself() {
        for label in KNOWN_LABELS {
            assert_eq!(known_label(label), Some(*label));
        }
    }

    #[test]
    fn an_unrecognized_label_is_none_not_a_guess() {
        assert_eq!(
            known_label("some_future_label_this_build_does_not_know"),
            None
        );
    }
}
