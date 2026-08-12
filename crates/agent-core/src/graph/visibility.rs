//! 红线 10 的**结构面**：每个槽位跨 agent 时朝哪个方向可读。
//!
//! 这个文件只回答一个问题：**「别的 agent 能不能读这个槽位、从哪个方向读」**。
//! 「一个槽位怎么称呼、它没有值时是什么」在 [`slot`](super::slot)，「谁来建它」在
//! [`build`](super::build)，「谁来读它」在 `command/cross_read.rs`。
//!
//! ## 为什么方向要写成一张穷举表
//!
//! 红线 10 的完整论证是：整棵 agent 树在同一个 store 里，**谁都物理可达**，依赖图
//! 必须靠 API 约束保持是树；而「两个方向可读的 slot 集合不相交 + 图恒为树」合起来才
//! 让环在结构上不可能。
//!
//! 集合不相交这件事，靠人记是记不住的——加一个槽位时忘了给它站队，它就默认落进
//! 「谁都能读」或者「谁都不能读」，前者破坏不相交、后者悄悄砍掉一条本该有的边。
//! 所以 [`Slot::visibility`] 是**穷举 match，一个 `_` 通配都没有**：新增槽位不显式
//! 站队就编译不过。这是红线 10 唯一能被编译器守住的部分，其余靠那两个读口的方向校验。
//!
//! ## 为什么不相交能推出无环
//!
//! 设 `U` 是 Upward 集合、`D` 是 Downward 集合。跨 agent 的边只有两种：
//! 后代读祖先的 `U` 槽位（边指向树根方向），祖先读后代的 `D` 槽位（边指向树叶方向）。
//! 一个环必须同时含有这两种边，于是环上存在某个槽位既被往上读又被往下读——
//! 那要求 `U ∩ D ≠ ∅`。反过来说，`U ∩ D = ∅` 时环不存在。
//!
//! 所以测试要断言的是**集合性质本身**（`U ∩ D = ∅`、三类构成一个划分），
//! 不是「子读父 Messages 能过 / 兄弟读被拒」这几个用例——用例过了不代表性质成立。

use super::slot::Slot;

/// 一个槽位在**跨 agent** 读取时的方向。
///
/// 注意「跨 agent」：一个 agent 读**自己**的槽位跟这个枚举无关（那不是跨 agent 读，
/// 也就不会在依赖图上产生跨 agent 的边）。`Private` 说的是「别的 agent 一律读不到」，
/// 不是「自己也读不到」——`Slot::TurnsUsed` 是 `Private`，本 agent 的转移表照读不误。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Visibility {
    /// 子可读父（往树根方向读）：`Session::read_ancestor` 放行的那一类。
    ///
    /// 收的是「子 agent 干活时真的需要上下文的那几样」——`docs/STATE-MODEL.md`
    /// 列的是 messages / config / skills，本仓的 `Slot` 表目前只有 `Messages`
    /// 落地（`config` / `system_base` / `skills_active` 还没有任何写入点，026 的
    /// 裁决：没被真实使用验证过的槽位跟没写一样）。它们补进 `Slot` 时**必须**
    /// 在这里显式站进 `Upward`。
    Upward,
    /// 父可读子（往树叶方向读）：`Session::read_descendant` 放行的那一类。
    ///
    /// 029 的「等所有子 agent 完成」是这个方向上第一个真实的汇聚 derived。
    Downward,
    /// 谁都读不到——一个 agent 的内部账本（本轮预算、消息号计数器、前缀镜像、
    /// 工具槽）。默认落这一类是刻意的：**开放一个方向要有理由，封闭不需要**。
    Private,
}

impl Slot {
    /// 这个槽位跨 agent 时朝哪个方向可读。
    ///
    /// **穷举，无通配**（见模块文档）。新增槽位时编译器会在这里逼你回答
    /// 「谁能读它」——漏答的代价是红线 10 破掉，而破掉之后不报错。
    pub fn visibility(self) -> Visibility {
        match self {
            // —— 往上：子 agent 干活要的上下文 ————————————————
            //
            // 子读父的消息历史是决策 3 兑现「子读父是一次 get」的那一下：不需要
            // 任何消息传递机制，走依赖图自动追踪、自动失效。
            Slot::Messages => Visibility::Upward,
            // `SkillsActive` 是 STATE-MODEL §「读取边界」点名 `read_ancestor` 读的
            // 「skills」那一样（039 让它落地）：skill 是上下文资产，子 agent 若要
            // 继承父的激活集，走的就是这条往上的边。M5 里子 agent 各有各的（空）
            // 激活集、各自注入，这条边还没有真实读者——但它跟 `Messages` 一样属于
            // 「子干活要的上下文」，站队要按语义、不按当下有没有用到（模块文档：
            // 开放一个方向要有理由，而 STATE-MODEL 已经给了理由）。
            Slot::SkillsActive => Visibility::Upward,
            // `HostTools`（073）跟 `SkillsActive` **同一类**：宿主注入的声明是这个
            // 会话的上下文资产，只写在 root 头上（`Session::declare_host_tools`），
            // 子 agent 要知道「这个会话里宿主给了哪些工具」就得往根的方向读。
            // 站队按语义、不按当下有没有读者（模块文档：开放一个方向要有理由）——
            // 而理由跟 skill 是同一条：注入的能力对整棵树可见，它属于这个会话，
            // 不属于某一个 agent 的内部账本。
            Slot::HostTools => Visibility::Upward,
            // `HostSkills`（064）跟上面两条同一类，而且它比 `HostTools` 更贴
            // `SkillsActive`：注入的 skill 就是「这个会话有哪些 skill 可用」，
            // 而激活集本来就是往上读的。两者站在同一边，子 agent 要么两样都读得到、
            // 要么两样都读不到——分开站队会造出「知道它被激活了、却查不到它是谁」。
            Slot::HostSkills => Visibility::Upward,
            // `DisabledBuiltins`（076）跟上面三条**同一边**，而且它必须是同一边：
            // 子 agent 不单独配这个开关，整棵树共用会话级的那一份（076 用户拍板，
            // `spawn` 一行没改）。让它 `Private` 就等于说「这是某一个 agent 的内部
            // 账本」——而它决定的是整棵树看得见哪些内置工具，那是会话级的事实。
            Slot::DisabledBuiltins => Visibility::Upward,
            // `PrefixChunks`（134）跟上面四条同一类，判据也是同一个：**它是会话级
            // 的事实，不是某一个 agent 的内部账本**。它只写在 root 头上
            // （`Session::set_prefix_chunks`），子 agent 要知道「这个会话开局定下了
            // 哪些 system 前缀」就得往根的方向读。让它 `Private` 就等于说这是
            // root 一个人的账——而它描述的是整个会话的开局条件。
            // 站队按语义、不按当下有没有读者（模块文档：开放一个方向要有理由）。
            Slot::PrefixChunks => Visibility::Upward,
            // `HostPrefix`（154，决策 31）跟上面五条同一类、同一条判据：宿主经
            // `capabilities.prefix` 声明的开局块只写在 root 头上
            // （`Session::declare_host_prefix`），是这个会话的上下文资产，不是
            // 某一个 agent 的内部账本。子 agent 要知道「这个会话开局声明了哪些
            // 开局块」就得往根的方向读——跟 `HostTools` 是同一个「声明是会话级
            // 事实」的判据（模块文档：站队按语义、不按当下有没有读者）。
            Slot::HostPrefix => Visibility::Upward,

            // —— 往下：父 agent 要知道子干完了没 ————————————————
            //
            // `Status` 是 029 的汇聚 derived 唯一要读的东西（「等所有子完成」）。
            Slot::Status => Visibility::Downward,
            // `ToolsAllowed` 同时是**活名单**：非 `Null` 才算这个 agent 活着
            // （`Session::is_live`）。汇聚 derived 要先知道「有哪些活着的子」才能
            // 去读它们的 `Status`，所以这一条边也是往下的。
            Slot::ToolsAllowed => Visibility::Downward,
            // `PrefixAllowed`（144）站 `ToolsAllowed` 同一边，但不是同一条理由：
            // 它**不是**活名单，判活继续只看 `ToolsAllowed`（模块文档：一个槽位
            // 只能站一边，双重身份是 `ToolsAllowed` 一家的事）。站 Downward 是因为
            // 它描述的是「这个子 agent 出生时被授予了什么」——跟 `Status` 一样是
            // **关于子的事实**，观测/父侧汇聚要查一个子拿到了哪些开局产物，读的
            // 是子自己头上的这个槽位，不是子要往上继承的会话级上下文（那是
            // Upward 那一组的判据）。
            Slot::PrefixAllowed => Visibility::Downward,

            // —— 私有：一个 agent 的内部账本 ————————————————
            //
            // 工具槽是本 agent 这一轮的在飞现场，父 agent 要的是结论（`Status`）
            // 不是现场；四个预算计数与消息号计数器是 per-agent 的账，跨 agent 读
            // 它们只会长出「把别人的预算算进自己的」这类账目错误；前缀镜像是
            // adapter 的比对材料，连本 agent 都只是原样存取。`SendPlan`（100）
            // 跟 `PrevPrefix` 同一类：它描述的是「这个 agent 这一轮发送侧的账本」，
            // 不是子 agent 干活要继承的上下文，也不是父 agent 要等的结论。压缩状态
            // 若真要跨 agent 共享，那是需要新理由的新决策（模块文档：开放一个方向
            // 要有理由，封闭不需要），不是这里的默认开放。`PrevSendPlan`（103）跟
            // `PrevPrefix` 是同一件事的两半，站队理由跟 `PrevPrefix` 完全相同。
            // `Summaries`（107）跟 `SendPlan` **必须站同一边**：摘要正文是边界那一
            // 侧的账（`SendPlan` 里的引用指向它），两者分开站队就会造出「子 agent
            // 读得到父的摘要引用、却查不到正文」，投影那边只能把边界作废——一个
            // 只在跨 agent 取料时才浮出来的静默降级。子 agent 要继承父的上下文走的
            // 是 `Messages` 那条上读边（完整历史，没被压过），不该经由压缩状态。
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

    /// **集合性质本身**（issue 028 的「注意」点名要的那条，不是几个用例）：
    /// 三类构成 `Slot::ALL` 的一个划分——两两不相交，并集是全集。
    ///
    /// `U ∩ D = ∅` 是「环在结构上不可能」的前提（模块文档里的证明），
    /// 所以它必须是一条断言，而不是一句注释。
    #[test]
    fn the_three_visibilities_partition_every_slot() {
        let up = slots_with(Visibility::Upward);
        let down = slots_with(Visibility::Downward);
        let private = slots_with(Visibility::Private);

        // 不相交：任一槽位只可能出现在一类里。
        for slot in &up {
            assert!(
                !down.contains(slot),
                "{slot:?} 同时可上读可下读——环就有可能了"
            );
            assert!(!private.contains(slot), "{slot:?} 落进了两类");
        }
        for slot in &down {
            assert!(!private.contains(slot), "{slot:?} 落进了两类");
        }

        // 并集是全集：没有槽位漏掉分类。
        assert_eq!(up.len() + down.len() + private.len(), Slot::ALL.len());
    }

    /// 两个方向都非空——空集当然不相交，那样的「不相交」什么都没保证。
    #[test]
    fn both_directions_are_actually_used() {
        assert!(!slots_with(Visibility::Upward).is_empty());
        assert!(!slots_with(Visibility::Downward).is_empty());
    }

    /// 钉住当下的具体归属：改动任何一条都得先在这里改，顺便解释为什么。
    #[test]
    fn the_current_assignment_is_pinned() {
        // 顺序 = `Slot::ALL` 里的相对次序（`slots_with` 保序过滤）：Messages 在最前，
        // 039 追加的 SkillsActive、073 追加的 HostTools、064 追加的 HostSkills、
        // 076 追加的 DisabledBuiltins、134 追加的 PrefixChunks、154 追加的
        // HostPrefix 在末尾。七者都是「子干活要的会话级上下文」（往上读）。
        assert_eq!(
            slots_with(Visibility::Upward),
            vec![
                Slot::Messages,
                Slot::SkillsActive,
                Slot::HostTools,
                Slot::HostSkills,
                Slot::DisabledBuiltins,
                Slot::PrefixChunks,
                Slot::HostPrefix,
            ]
        );
        // 144 追加 PrefixAllowed，跟 ToolsAllowed 同一边（理由不同，见上面的
        // match 分支注释）。
        assert_eq!(
            slots_with(Visibility::Downward),
            vec![Slot::Status, Slot::ToolsAllowed, Slot::PrefixAllowed]
        );
    }
}
