//! 红线 10 的**结构面**：一个槽位**别的 agent 读不读得到**。
//!
//! 这个文件只回答这一个问题。「一个槽位怎么称呼、它没有值时是什么」在
//! [`slot`](super::slot)，「谁来建它」在 [`build`](super::build)，「谁来读它」在
//! `command/cross_read.rs`。
//!
//! ## 决策 35 之前这里问的是「从哪个方向读」
//!
//! 旧红线 10 只允许两个方向（后代读祖先 / 祖先读后代），论证是：
//! 「`U ∩ D = ∅` + 图恒为树 ⇒ 环不可能」。**那个论证的前提在这个仓里从来没成立过**
//! ——它假设跨 agent 读会在依赖图上建边，而 `cross_read.rs` 的读走
//! [`Session::peek`](crate::command::Session) → `store.get`，是**命令层的非追踪读，
//! 一条边都不建**。建边只发生在 derived 的 read fn 里调 `args.get`，而在决策 35
//! 之前，那样的调用在生产代码里只有 [`build`](super::build) 一处、读的还是**自己
//! agent** 的槽位。
//!
//! 所以方向约束防的是一类当时还不存在的边。决策 35 把它去掉，**横读全开**：
//! 兄弟之间互相看得见、说得上话，判据只剩「这个槽位是不是内部账本」。
//!
//! ## 那无环靠什么
//!
//! 靠**边只许指向 primitive**——跨 agent 的 `args.get` 只能拿 [`Slot`] 去构
//! [`AtomKey::Agent`](super::AtomKey)，而那永远落在 source family 上
//! （[`build`](super::build) 里 source 与 derived 是两张按不同键类型索引的表，
//! 「快照只存 primitive」是**类型上的事实**）。primitive 没有出边，于是跨 agent 的边
//! 全是长度 1 的悬边，绕不回来。
//!
//! 这条判据的**落点不在本文件**：本文件管的读一条边都不建，证不了它。第一条真的
//! 跨 agent 的边由 `srv:agent/await` 的 derived 建出来（issue 212），断言在那里。
//! **在这里留一句是因为读到这个文件的人多半正想问「那环怎么办」。**
//!
//! ## 为什么站队要写成一张穷举表
//!
//! 加一个槽位时忘了给它站队，它就默认落进「谁都能读」或「谁都不能读」，
//! 前者把内部账本开出去、后者悄悄砍掉一条本该有的边。所以 [`Slot::visibility`] 是
//! **穷举 match，一个 `_` 通配都没有**：新增槽位不显式站队就编译不过。
//!
//! **横读全开之后这条纪律更要紧了。** 以前站错方向最多是多一条单向边；现在站错
//! 就是**所有人都读得到**。

use super::slot::Slot;

/// 一个槽位在**跨 agent** 读取时开不开放。
///
/// 注意「跨 agent」：一个 agent 读**自己**的槽位跟这个枚举无关。`Private` 说的是
/// 「**别的** agent 一律读不到」，不是「自己也读不到」——[`Slot::TurnsUsed`] 是
/// `Private`，本 agent 的转移表照读不误，`srv:agent/self`（issue 208）读的也是它。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    /// 别的 agent 读得到，**不限方向**（决策 35）：祖先、后代、兄弟都行。
    Shared,
    /// 谁都读不到——一个 agent 的内部账本（本轮预算、消息号计数器、前缀镜像、
    /// 工具槽、压缩账、收件箱）。默认落这一类是刻意的：**开放要有理由，封闭不需要**。
    Private,
}

impl Slot {
    /// 这个槽位别的 agent 读不读得到。
    ///
    /// **穷举，无通配**（见模块文档）。新增槽位时编译器会在这里逼你回答
    /// 「别人能不能读它」——漏答的代价是红线 10 破掉，而破掉之后不报错。
    pub fn visibility(self) -> Visibility {
        match self {
            // —— 开放：这个会话的共享事实 ————————————————
            //
            // 子读父的消息历史是决策 3 兑现「子读父是一次 get」的那一下。
            //
            // **决策 35 起它也向兄弟和父方向开**，而这一格有真代价：读一个别人的
            // 全程 transcript 进自己的 prompt，一次就能把一轮成本翻几倍。
            // core 这一层放行，**工具层不给模型开按槽位读它的入口**——模型侧要正文
            // 有 `srv:agent/collect`（领结果）和 `srv:agent/send`（要什么问什么），
            // 两条都有边界。这个分层是刻意的，别在加工具时顺手把它开出去。
            Slot::Messages => Visibility::Shared,
            // `SkillsActive`（039，141 起只剩壳）、`HostTools`（073）、
            // `HostSkills`（064）、`DisabledBuiltins`（076）、`PrefixChunks`（134）、
            // `HostPrefix`（154，决策 31）——六者同一条判据：**它们是会话级的事实，
            // 不是某一个 agent 的内部账本**。多数只写在 root 头上，描述的是「这个
            // 会话有哪些能力 / 开局定下了什么」，对整棵树可见。
            //
            // 站队按语义、不按当下有没有读者（模块文档：开放要有理由，而
            // STATE-MODEL §「读取边界」已经给了理由）。
            Slot::SkillsActive
            | Slot::HostTools
            | Slot::HostSkills
            | Slot::DisabledBuiltins
            | Slot::PrefixChunks
            | Slot::HostPrefix => Visibility::Shared,
            // `Status` 是「这个 agent 此刻走到哪了」——**关于它的事实，给别人看的
            // 结论**。029 的汇聚要读它，`srv:agent/status`（207）经 `agent_tree()`
            // 渲染的也是它，`srv:agent/await`（212）订阅的还是它。
            Slot::Status => Visibility::Shared,
            // `ToolsAllowed` 同时是**活名单**：非 `Null` 才算这个 agent 活着
            // （`Session::is_live`）。要知道「有哪些活着的 agent」就得读得到它。
            Slot::ToolsAllowed => Visibility::Shared,
            // `PrefixAllowed`（144）跟 `Status` 同一条理由（**关于这个 agent 的
            // 事实**），不是活名单——判活继续只看 `ToolsAllowed`（一个槽位只能
            // 站一边，双重身份是 `ToolsAllowed` 一家的事）。
            Slot::PrefixAllowed => Visibility::Shared,

            // —— 私有：一个 agent 的内部账本 ————————————————
            //
            // 工具槽是本 agent 这一轮的在飞现场，别人要的是结论（`Status`）不是现场；
            // 四个预算计数与消息号计数器是 per-agent 的账，跨 agent 读它们只会长出
            // 「把别人的预算算进自己的」这类账目错误；前缀镜像是 adapter 的比对材料，
            // 连本 agent 都只是原样存取。`SendPlan`（100）/`PrevSendPlan`（103）/
            // `Summaries`（107）是压缩边界那一侧的账，三者**必须站同一边**：分开站队
            // 会造出「读得到引用、却查不到正文」，投影那边只能把边界作废——一个只在
            // 跨 agent 取料时才浮出来的静默降级。要别人的上下文走 `Messages`，
            // 要别人说句话走 `srv:agent/send`。
            Slot::ToolSlots
            | Slot::PrevPrefix
            | Slot::NextMessageId
            | Slot::TurnsUsed
            | Slot::MaxTurns
            | Slot::RetriesUsed
            | Slot::MaxRetries
            | Slot::ExecutionProfile
            | Slot::SendPlan
            | Slot::PrevSendPlan
            | Slot::Summaries => Visibility::Private,
            // `Inbox`（205）：**发得进去 ≠ 读得出来**。投递是一条命令（写），
            // 不是读——A 能往 B 的收件箱放话，但读不到 B 的收件箱，包括自己投的
            // 那条被没被消费。开成 `Shared` 就等于让「谁给谁发过什么」成为所有人
            // 都看得到的东西，而那不是任何一个已知需求要的。要确认对方收到没有，
            // 等他回一条（决策 35 §五点名不做投递回执）。
            Slot::Inbox => Visibility::Private,
            // `Notes`（209）：模块文档那条规矩的一次直接应用——**开放要有理由，
            // 封闭不需要**，而这一格提不出理由。它是这个 agent 写给自己的东西；
            // 子 agent 要父的上下文走 `Messages` 那条边，要谁说句话走
            // `srv:agent/send`，两条都有边界、都看得见来源。
            //
            // 开成 `Shared` 的代价具体是：横读全开之后那不是「子继承父」，是
            // **所有人都读得到**，于是「一个 agent 改一个 key」变成影响别人下一轮
            // prompt 的事，而模型完全看不到这条因果。
            Slot::Notes => Visibility::Private,
            // `AwaitingOn`（212）：**等待图是内部账本**。它是「这个 agent 此刻在
            // 等谁」——跟 `Inbox` 同一条判据：开成 `Shared` = 所有人都订阅得到
            // 「谁在等谁」，而那不是任何一个已知需求要的。查环要遍历它，但那是
            // core 内部的命令层在做，不经过跨 agent 读口。
            Slot::AwaitingOn => Visibility::Private,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots_with(v: Visibility) -> Vec<Slot> {
        Slot::ALL
            .into_iter()
            .filter(|s| s.visibility() == v)
            .collect()
    }

    /// **集合性质本身**（不是几个用例）：两类构成 `Slot::ALL` 的一个划分。
    ///
    /// 决策 35 之前这里断言的是 `U ∩ D = ∅`（三类划分），因为那是「环不可能」的
    /// 前提。方向去掉之后前提换了地方（见模块文档「那无环靠什么」，断言在 212），
    /// 这里剩下的性质是**每个槽位恰好站一边**——它守的是「加槽位时没漏答」，
    /// 而穷举 match 只能保证「答了」，保证不了「只答了一次」。
    #[test]
    fn the_two_visibilities_partition_every_slot() {
        let shared = slots_with(Visibility::Shared);
        let private = slots_with(Visibility::Private);

        for slot in &shared {
            assert!(!private.contains(slot), "{slot:?} 同时站了两边");
        }
        assert_eq!(
            shared.len() + private.len(),
            Slot::ALL.len(),
            "有槽位一边都没站"
        );
    }

    /// 两类都非空——空集当然不相交，那样的「划分」什么都没保证。
    #[test]
    fn both_classes_are_actually_used() {
        assert!(!slots_with(Visibility::Shared).is_empty());
        assert!(!slots_with(Visibility::Private).is_empty());
    }

    /// 钉住当下的具体归属：改动任何一条都得先在这里改，顺便解释为什么。
    ///
    /// 顺序 = `Slot::ALL` 里的相对次序（`slots_with` 保序过滤）。
    #[test]
    fn the_current_assignment_is_pinned() {
        assert_eq!(
            slots_with(Visibility::Shared),
            vec![
                Slot::Messages,
                Slot::Status,
                Slot::ToolsAllowed,
                Slot::SkillsActive,
                Slot::HostTools,
                Slot::HostSkills,
                Slot::DisabledBuiltins,
                Slot::PrefixChunks,
                Slot::PrefixAllowed,
                Slot::HostPrefix,
            ]
        );
        assert_eq!(
            slots_with(Visibility::Private),
            vec![
                Slot::ToolSlots,
                Slot::PrevPrefix,
                Slot::NextMessageId,
                Slot::TurnsUsed,
                Slot::MaxTurns,
                Slot::RetriesUsed,
                Slot::MaxRetries,
                Slot::ExecutionProfile,
                Slot::SendPlan,
                Slot::PrevSendPlan,
                Slot::Summaries,
                // 205 追加 Inbox：发得进去 ≠ 读得出来。
                Slot::Inbox,
                // 209 追加 Notes：模型自己的草稿纸，只有它自己看得到。
                Slot::Notes,
                // 212 追加 AwaitingOn：等待图是内部账本，查环的人是 core 自己。
                Slot::AwaitingOn,
            ]
        );
    }
}
