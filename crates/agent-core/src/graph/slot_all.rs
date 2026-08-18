//! 槽位**名册**：[`Slot::ALL`]——「这个会话每个 agent 都要建哪些槽位」。
//!
//! 隔壁 [`slot`](super::slot) 回答的是「一个槽位怎么称呼、它为什么存在」，
//! 这个文件回答的是「一共有哪些」。209 拆开（红线 9：`slot.rs` 顶破 300 行），
//! 而拆的位置正是 `graph/mod.rs` 那句用「以及」连起来的描述——**说不清一句不含
//! 「和/以及」的话，就是两件事**。
//!
//! 这份名册是承重的：`Session::new` 建图、`Session::primitives` 出快照都遍历它。
//! 漏一格的症状不是编译错误，是快照里少一个键，而缺键在恢复时按
//! [`Slot::default_value`] 补默认值——链通、值错、不报错。

use super::slot::Slot;

impl Slot {
    /// 一个 agent 的全部 source 槽位。`Session::new` 建图、`Session::primitives`
    /// 出快照都用它——**新增槽位只要加进这个数组，两条路径自动跟上**，
    /// 忘了改其中一条正是「快照缺一块」的来源。
    ///
    /// 新槽位**追加在末尾**：旧快照里找不到新键，按 [`Slot::default_value`] 落值
    /// （schema 演进白拿的那一条），而追加不改动既有槽位的相对次序，
    /// 快照的排序输出因此在版本之间是稳定的。
    pub const ALL: [Slot; 23] = [
        Slot::Messages,
        Slot::Status,
        Slot::ToolSlots,
        Slot::PrevPrefix,
        Slot::NextMessageId,
        Slot::TurnsUsed,
        Slot::MaxTurns,
        Slot::RetriesUsed,
        Slot::MaxRetries,
        Slot::ToolsAllowed,
        Slot::SkillsActive,
        Slot::HostTools,
        Slot::HostSkills,
        Slot::DisabledBuiltins,
        Slot::ExecutionProfile,
        Slot::SendPlan,
        Slot::PrevSendPlan,
        Slot::Summaries,
        Slot::PrefixChunks,
        // 144 追加 PrefixAllowed。
        Slot::PrefixAllowed,
        // 154 追加 HostPrefix。
        Slot::HostPrefix,
        // 205 追加 Inbox（决策 35）。
        Slot::Inbox,
        // 209 追加 Notes（决策 35 §三）。
        Slot::Notes,
    ];
}