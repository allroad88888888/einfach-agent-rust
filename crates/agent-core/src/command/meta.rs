//! 一条 `Entry` 的元数据，以及 agent 侧那三个日志类型的别名。
//!
//! 009 把 `Entry` 泛型化成 `Entry<K, V, M>`：`turn_id` / `epoch` / `label` 全是 agent
//! 词汇，而 `History` 住在 `agent-store`，那个 crate 不许 import `agent-core`
//! （ARCHITECTURE §包结构）。整组字段因此成为泛型 `M`——[`EntryMeta`] 就是 026
//! 把它填上的那一份。

use serde::{Deserialize, Serialize};

use crate::engine::epoch::Epoch;
use crate::graph::AtomKey;
use crate::value::atom_value::AgentValue;

/// 这一步**撤销起来要做什么**（决策 199 §九，取代原来的 `barrier: bool`）。
///
/// 两态不够用，因为「不挡 undo」实际上是两件不同的事：这一步压根没碰外部世界
/// （`user_input` / `provider_done` 这类），和这一步碰了但工具把还原函数交回来了。
/// 两者在日志里长得一样，可**还原函数是闭包、活在进程里**：崩溃恢复之后钩子表是
/// 空的，而落盘的这一位是持久的。分不开就会在恢复之后照「不挡」走，**静默跳过
/// 一次真实副作用**。
///
/// 这条同时把边界画明白：**状态的逆跨进程有效**（journal 的 prev/next 是数据），
/// **外部世界的逆不跨进程**（它是闭包）。
///
/// 落盘用得上 `Deserialize`（`EntryMeta` 自己因为 `label: &'static str` 不能 derive，
/// 见类型文档；这个枚举没有那个问题，于是 `agent-runtime` 的 `PersistedMeta` 直接
/// 复用它，不另抄一份三态）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Undoability {
    /// 没碰外部世界——状态回滚就够了（今天绝大多数 entry）。
    StateOnly,
    /// 碰了，且交了还原函数。钩子表按 [`Entry::seq`](agent_store::Entry::seq) 查，
    /// **不按 `ToolCallId`**：`seq` 由 `History` 铸造、严格递增、本来就在 `Entry` 上，
    /// 而往 [`EntryMeta`] 塞一个 `call_id` 要动落盘 schema（199 §九：能不加字段就不加）。
    Hooked,
    /// 碰了，没交还原函数——**屏障**，undo 走到这里停下来问用户。
    Blocked,
}

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
    /// 这一步撤销起来要做什么（199 §九的三态，2026-08-17 之前是 `barrier: bool`）。
    ///
    /// 谁来置：宿主在派发工具时调
    /// [`Session::mark_no_undo`](super::Session::mark_no_undo)（→ [`Undoability::Blocked`]）
    /// 或 [`Session::mark_hooked`](super::Session::mark_hooked)（→ [`Undoability::Hooked`]）
    /// ——**core 没有工具表**，也不认识还原函数的类型，工具交没交回还原函数只有
    /// 宿主知道，core 现造一个结论等于编造（002 合并时的裁决，199 §二原样沿用）。
    pub undoability: Undoability,
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

/// 屏障谓词：**只有 [`Undoability::Blocked`] 挡路**。喂给 `History::undo_turn`。
///
/// [`Undoability::Hooked`] 不挡：它有还原函数，撤得掉。它撤不掉的那种情况
/// （钩子跑失败 / 钩子随进程重启没了）判不出来——那要真的调一次钩子才知道，
/// 而 `History` 的谓词只看得见 `&EntryMeta`。所以那一档停在
/// [`undo_hook`](super::undo_hook) 的逐条循环里，不在这里。
pub(crate) fn is_barrier(meta: &EntryMeta) -> bool {
    matches!(meta.undoability, Undoability::Blocked)
}

/// 全部合法的 `label` 取值——[`transitions::label_of`](super::transitions) 的九个 +
/// `Session` 会话级命令的四个（`begin_turn` / `set_max_turns` / `set_max_retries` /
/// `clear_prev_prefix`）+ 028 的两条树形命令（`spawn_child` / `despawn_child`）+
/// `activate_skill` / `deactivate_skill`（039 加、141 删了写入点——**标签留着**：
/// 老 journal 里真有这两种 entry，`recover` 不认识的标签会硬失败，见下面「不变量」
/// 那段）+ 073 的宿主注入声明（`declare_host_tools`）+ 064/076 的两条同款声明
/// （`declare_host_skills` / `disable_builtins`）+ 100 的 `SendPlan` 整体替换
/// （`replace_send_plan`——104 的 `advance_boundary` 生效时也是调它，复用同一个
/// 标签，不另开一格）+ 107 的摘要回写（`apply_summary`）+ 134 的会话开局前缀
/// （`prefix_init`）+ 154 的宿主开局块声明（`declare_host_prefix`——决策 31，跟
/// `declare_host_tools` 同一条理由）。
///
/// **134 为什么不叫 `set_prefix_chunks`**：label 记的是「当时发生了什么」，而这一步
/// 发生的事是「这个会话的开局前缀在这里定下来了」——它按设计只发生一次、在第一轮
/// 之前。叫成命令名会让人以为它跟别的槽位写入一样可以反复出现在时间线上。
///
/// **107 为什么不复用 `replace_send_plan`**：104 复用它的理由是「这条命令在状态层
/// 做的事就是整体换掉那一个槽位的值」；`apply_summary` 不是——它在一条 entry 里同时
/// 写 `Slot::Summaries` 和 `Slot::SendPlan`，而 label 要回答的是「当时发生了什么」
/// （[`EntryMeta::label`]）。挂着「换了个发送计划」的名字去审计一条同时存进一份
/// 摘要正文的 entry，时间线上就少了一件真的发生过的事（109 要展示的正是它）。
/// 这是一个**封闭的、有限的编译期常量集**（`EntryMeta.label` 的文档注释）。
///
/// **加一条就是一次协议变更**：用新代码写出来的会话文件，旧二进制打开时会在这里
/// 认不出这个标签，`recover` 硬失败而不是编一个假的凑合用（[`known_label`]）。
///
/// **不变量：`label_of` 的取值域必须是这张表的子集。** 105 的
/// `compact_done` / `compact_failed` 因此现在就在这里，尽管那两格转移这一版
/// 一个 primitive 都不写（写回是 107），落不出带这个标签的 entry——等到 107 真的
/// 写状态时才补，中间就有一版「能产出、认不出」的代码，而它炸的地方是恢复，
/// 离改动点最远。
const KNOWN_LABELS: &[&str] = &[
    "user_input",
    "provider_done",
    "provider_failed",
    "tool_result",
    "tool_failed",
    "timeout",
    "compact_done",
    "compact_failed",
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
    "replace_send_plan",
    "apply_summary",
    "prefix_init",
    "declare_host_prefix",
    // M20（决策 35）：205 的三条收件箱命令、209 的草稿纸写入、214 的唤醒转移。
    //
    // **漏一条的症状是「用过这个功能的会话恢复不回来」**——`recover` 撞
    // `UnknownLabel` 直接硬失败，而不是退化成别的什么。206 落地时就漏了
    // `"deliver"`，一直到 211 的独立测试 agent 造一个「带留言的会话落盘再恢复」
    // 的场景才浮出来：**当时没有任何测试同时做过「用 send」和「持久化往返」
    // 这两件事**。新增一条命令就要往这张表里加一行，忘了不会编译错。
    "deliver",
    "drain_now",
    "drain_next_turn",
    "set_note",
    "wake",
    "await_agent",
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
