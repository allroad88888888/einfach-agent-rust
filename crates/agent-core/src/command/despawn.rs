//! [`Session::despawn_child`]：把一棵子树从图上拆掉。
//!
//! **这是 019 三条硬约束的第一次真实执行。** 那三条是实测钉出来的，不是推导出来的
//! （`docs/issues/019-applier-recreate.md` §「derived 重连语义的真相」），逐条对应到
//! 本文件的一段代码：
//!
//! | 019 硬约束 | 这里怎么落 |
//! |---|---|
//! | 1. 逐出自叶向根（先 derived 后 primitive、子树递归） | [`Session::live_subtree_leaf_first`] 给顺序；每个 agent 先销自己的 `ToolsConverged` 再销 primitive |
//! | 2. 逐出状态驱动（还有读者就拒绝） | teardown 先把 `ToolsAllowed` 写成 `Null`（移出活名单），逐出前逐个 atom 问 `direct_dependents`，还有外人读就整条命令拒绝、一个字节不改 |
//! | 3. 重建保证 atom 回来、不保证值回来 | teardown 是一条**真的 command**：每个槽位的活值当场被记成 `prev`，undo 才拿得回来。不记就是「链通、值错、不报错」 |
//!
//! ## 为什么 `ToolsAllowed` 不逐出
//!
//! 每个被 despawn 的 agent 留一个 `ToolsAllowed = Null` 的**墓碑**，两个理由：
//!
//! 1. **号不复用**：spawn 铸号取的是 family 键空间里的最大号（`spawn.rs`），
//!    墓碑在，号就单调。全逐出的话 despawn 完再 spawn 会拿回同一个 `AgentId`，
//!    审计时间线上就有两个同名 agent，而 undo 日志的键正是这个 id。
//! 2. **它就是 019 第 2 条说的「活名单」**：029 的汇聚 derived（「等所有子完成」）
//!    要先知道有哪些活着的子，读的就是各个子的 `ToolsAllowed`——那条读边一直在，
//!    引擎本来也不会让它被逐出（`AtomFamily::evict` 有下游就返回 false）。
//!    换句话说墓碑不是我们额外留的，是逐出规则自己留下的。
//!
//! 代价是每个死掉的子 agent 留一个 atom 而不是零个：十一分之一的残留，
//! 换「id 一生只属于一个 agent」和一条不会被引擎拒绝的逐出路径。
//!
//! ## undo 撤销 despawn，redo **不**重新逐出
//!
//! 逐出不产生 `Change`（019 第 5 条），所以它不在日志里，redo 也就无从反演。
//! `undo → redo` 之后，子树的**值**和 despawn 之后完全一致（全是默认值、不在活
//! 名单上），只有 atom 还占着内存。这是刻意的不对称：**日志管值，不管驻留**。
//! 反过来做（让 redo 顺手 evict）就是让一次重放去执行一件日志里没写的事，
//! 而那件事还可能被引擎拒绝——undo/redo 不允许失败。真要再回收一次，
//! 再下一条 `despawn_child` 命令，它会重新记账。
//!
//! ## despawn 不管在飞的东西
//!
//! 子 agent 被拆掉时可能还有工具在飞。它们的回执照常到达，被 `Session::step` 的
//! 活性闸静默丢掉——和 epoch 闸挡过期回执是同一个道理（那是**正常现象**，
//! 每条都喊一声只会刷屏）。要连带取消在飞请求是 `Effect::CancelInFlight` 的事，
//! 而 epoch 是会话级的，按 agent 取消是另一回事（`Effect` 的文档注释早就写明）。

use agent_store::AtomId;

use crate::graph::{AtomKey, DerivedKey, Slot};
use crate::ids::AgentId;

use super::session::Session;

/// despawn 被拒的理由。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DespawnRefused {
    /// 不能拆会话的 root：它的生命周期就是会话本身的生命周期
    /// （STATE-MODEL §「并发」：一个 root agent + 它的整棵子树 = 一个 session）。
    Root,
    /// 这个 id 不在本会话这棵树上。
    NotInSession { agent: AgentId },
    /// 不在活名单上：从没 spawn 过、spawn 被 undo 撤了、或者已经 despawn 过一次。
    NotLive { agent: AgentId },
    /// **019 硬约束 2**：这个槽位还有子树之外的读者，逐出会被引擎拒绝
    /// （`AtomFamily::evict` 返回 false，`Store::destroy_atom` 直接 panic）。
    ///
    /// 拒绝发生在**动手之前**，所以撞上它时状态一个字节都没改：半拆的子树比
    /// 拆不掉更糟——那是一个图上还在、值已经清空的 agent，而没有任何日志说过
    /// 它被清空了。
    StillRead { key: AtomKey },
}

/// 一次 despawn 拆掉了什么。
#[derive(Clone, PartialEq, Eq, Debug)]
#[must_use = "拆了多少东西是要进审计与 UI 时间线的"]
pub struct DespawnReport {
    /// 被拆掉的 agent，**自叶向根**——逐出就是按这个顺序做的（019 硬约束 1）。
    pub agents: Vec<AgentId>,
    /// 真的从图上逐出的 atom 数。每个 agent 会留下一个 `ToolsAllowed` 墓碑
    /// （见模块文档），所以它等于「子树的 atom 总数 − `agents.len()`」。
    pub atoms_evicted: usize,
}

impl Session {
    /// 拆掉 `child` 和它底下的整棵活子树。
    ///
    /// # 四步，顺序不能换
    ///
    /// 1. **校验**：不是 root、在本会话里、还活着；
    /// 2. **预检读者**（019 硬约束 2）：子树里每个 atom 的下游，除了子树自己的
    ///    derived 之外必须是空的。有外人 → [`DespawnRefused::StillRead`]，
    ///    此时什么都没改；
    /// 3. **teardown command**（019 硬约束 3）：一条 `Entry`，把子树每个槽位写回
    ///    它的默认值——**活值在这一刻被记成 `prev`**。`ToolsAllowed` 写成 `Null`
    ///    就是「移出活名单」那一下；
    /// 4. **逐出**（019 硬约束 1）：自叶向根，每个 agent 先销 derived 再销
    ///    primitive，`ToolsAllowed` 留作墓碑。
    ///
    /// # 一条 Entry，不是每个 agent 一条
    ///
    /// 「自叶向根」管的是**逐出顺序**，不是日志的切分。整棵子树的 teardown 是
    /// 一次 `store.batch` = 一个 undo 步：拆一半的子树在任何时刻都不该是一个可以
    /// 停下来的状态，而两条 entry 之间正好就是一个可以停下来的位置。
    ///
    /// # undo 回来的是什么
    ///
    /// 被逐出的 atom 由 applier 的 `resolve`（get-or-create，就是
    /// `graph::source_atom`）按逻辑键重建，再灌回这条 entry 带的 `prev`——019 的
    /// 整条链第一次跨 agent 跑通。derived 不在日志里，它在下一次被读到时按逻辑键
    /// 重建并自动接回图（019 第 4 条）。
    pub fn despawn_child(&mut self, child: &AgentId) -> Result<DespawnReport, DespawnRefused> {
        if child == &self.agent {
            return Err(DespawnRefused::Root);
        }
        if !self.in_session(child) {
            return Err(DespawnRefused::NotInSession { agent: child.clone() });
        }
        if !self.is_live(child) {
            return Err(DespawnRefused::NotLive { agent: child.clone() });
        }

        let agents = self.live_subtree_leaf_first(child);
        let keys = self.subtree_keys(&agents);
        self.refuse_if_still_read(&agents, &keys)?;

        // 3. teardown：活值 → `prev`，槽位 → 默认值。记在**父** agent 名下——
        //    是它下的这道命令（029 里就是父 agent 的那次 tool call）。
        let owner = child.parent().expect("非 root 的 agent 一定有父");
        let torn = keys.clone();
        self.commit_as(&owner, "despawn_child", move |txn| {
            for key in torn {
                let default = key.default_value();
                txn.set_key(key, default);
            }
        });

        let atoms_evicted = self.evict_subtree(&agents, &keys);
        Ok(DespawnReport { agents, atoms_evicted })
    }

    /// 子树占用的全部 family 键，按 `agents` 给的自叶向根顺序分组、组内按键排序。
    ///
    /// 遍历的是 family 的键空间而不是 `Slot::ALL`：`AtomKey` 还有 `ToolCall` 那一支
    /// （M3 之后子 agent 的工具槽会长在那儿），照槽位表列会漏掉它们——漏掉的那个
    /// atom 既不会被 teardown 记进 `prev`，也不会被逐出，正是「链通、值错」。
    fn subtree_keys(&self, agents: &[AgentId]) -> Vec<AtomKey> {
        let all: Vec<AtomKey> = self.sources.borrow().iter().map(|(k, _)| k.clone()).collect();
        let mut out = Vec::new();
        for agent in agents {
            let mut mine: Vec<AtomKey> =
                all.iter().filter(|k| k.agent() == agent).cloned().collect();
            mine.sort();
            out.extend(mine);
        }
        out
    }

    /// 019 硬约束 2 的预检：子树之外还有人读，就整条命令拒绝。
    ///
    /// 子树**自己的** derived 不算外人——它们马上会被销掉，而且销的顺序就在
    /// primitive 之前。除此之外的任何下游边或订阅都意味着「有人还在读」：
    /// 029 的汇聚 derived 若还把这个子 agent 当活的，就会落在这里。
    fn refuse_if_still_read(
        &self,
        agents: &[AgentId],
        keys: &[AtomKey],
    ) -> Result<(), DespawnRefused> {
        let own_derived: Vec<AtomId> = agents
            .iter()
            .filter_map(|a| {
                self.derived
                    .borrow()
                    .get(&DerivedKey::ToolsConverged(a.clone()))
            })
            .collect();

        for key in keys {
            let Some(id) = self.sources.borrow().get(key) else {
                continue;
            };
            let outsiders = self
                .store
                .direct_dependents(id)
                .into_iter()
                .any(|dep| !own_derived.contains(&dep));
            if outsiders || self.store.has_subscribers(id) {
                return Err(DespawnRefused::StillRead { key: key.clone() });
            }
        }
        Ok(())
    }

    /// 019 硬约束 1：自叶向根，先 derived 后 primitive，`ToolsAllowed` 留作墓碑。
    ///
    /// 顺序反了引擎会当场教做人——`AtomFamily::evict` 有下游就返回 false（好），
    /// `Store::destroy_atom` 有反向边就直接 panic（坏）。本文件的单元测试把这两种
    /// 结局都钉了一遍。
    fn evict_subtree(&mut self, agents: &[AgentId], keys: &[AtomKey]) -> usize {
        let mut evicted = 0;
        for agent in agents {
            // 先销这个 agent 的 derived：它读着自己的 `ToolSlots`，不销掉的话
            // 下一行的 primitive 逐出会被拒。
            self.derived
                .borrow_mut()
                .evict(&self.store, &DerivedKey::ToolsConverged(agent.clone()));

            for key in keys.iter().filter(|k| k.agent() == agent) {
                if matches!(key, AtomKey::Agent(_, Slot::ToolsAllowed)) {
                    continue; // 墓碑，见模块文档
                }
                if self.sources.borrow_mut().evict(&self.store, key) {
                    evicted += 1;
                }
            }
        }
        evicted
    }
}

/// 只留**必须握着 `Session` 内脏**的两条：外部读者的拒绝路径要自己造一条读边，
/// 逐出顺序要直接问 family。其余（记 prev / 自叶向根 / 三种拒绝 / undo 重建）
/// 走公开面，住在 `tests/session_subagent_despawn.rs`。
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::command::spawn::ChildConfig;
    use crate::graph::{derived_atom, source_atom};

    use super::*;

    fn session_with_child() -> (Session, AgentId) {
        let mut s = Session::new(AgentId::root());
        let child = s
            .spawn_child(
                &AgentId::root(),
                ChildConfig { tools_allowed: vec![Arc::from("srv:fs/read")] },
            )
            .unwrap();
        (s, child)
    }

    /// **019 硬约束 1 的反面**：derived 还活着时，它读的那个 primitive 逐不掉
    /// ——引擎写死的顺序，不是我们的约定。`despawn_child` 之所以能成，
    /// 正是因为它先销 derived。
    #[test]
    fn a_slot_the_childs_derived_still_reads_cannot_be_evicted_first() {
        let (s, child) = session_with_child();
        let converged = derived_atom(
            &s.store,
            &s.sources,
            &s.derived,
            &DerivedKey::ToolsConverged(child.clone()),
        );
        let _ = s.store.get(converged); // 算一次，把反向边装上

        let key = AtomKey::Agent(child, Slot::ToolSlots);
        assert!(
            !s.sources.borrow_mut().evict(&s.store, &key),
            "还有下游 derived 时逐出必须被拒绝"
        );
    }

    /// **019 硬约束 2**：子树之外还有人读时，整条命令拒绝，而且一个字节都没改。
    ///
    /// 这个测试自己造了一个「外人」derived——它捕获了 `AtomId`（红线 4 的孪生条款
    /// 明令 derived 不许这么干），这里是刻意的：它的唯一职责是**持有一条读边**，
    /// 从建出来到测试结束都不会被重建。生产代码里的汇聚 derived 一律按逻辑键
    /// 现查 family（`graph/build.rs`）。
    #[test]
    fn an_outside_reader_refuses_the_whole_despawn() {
        let (mut s, child) = session_with_child();
        let watched = source_atom(
            &s.store,
            &s.sources,
            &AtomKey::Agent(child.clone(), Slot::Status),
        );
        let watcher = s.store.create_derived_ctx(move |args| args.get(watched));
        let _ = s.store.get(watcher); // 装上反向边

        let before = s.primitives();
        let history_len = s.history_len();

        let Err(DespawnRefused::StillRead { key }) = s.despawn_child(&child) else {
            panic!("还有外部读者时必须拒绝");
        };
        assert_eq!(key, AtomKey::Agent(child.clone(), Slot::Status));
        assert_eq!(s.primitives(), before, "拒绝时状态一个字节都不该改");
        assert_eq!(s.history_len(), history_len, "拒绝不该留下一条 entry");
        assert!(s.is_live(&child), "拒绝之后子 agent 还活着");
    }
}
