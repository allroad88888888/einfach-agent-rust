//! 树形查询：**「这个会话里现在有哪些 agent、谁是谁的孩子、谁还活着」**。
//!
//! spawn 的两道闸（深度、子数）、despawn 的子树遍历、跨 agent 读的方向校验，
//! 三处都要问这几个问题，所以答案收在一处。
//!
//! ## 树的形状不存在任何一个 atom 的值里
//!
//! 父子关系**只在 [`AgentId`] 的路径里**（`ids/agent.rs` 的模块文档解释了为什么
//! 不能存 parent 指针：判定读 store，而 undo 正在回滚 store，会绕成死结）。
//! 这里唯一从 store 拿的是「哪些键存在」和「`ToolsAllowed` 是不是 `Null`」——
//! 前者是 family 的键空间，后者是**活名单**本身。
//!
//! ## 活着 = `ToolsAllowed` 不是 `Null`
//!
//! 一个 agent 的 atom 在图上，不等于它活着。三种情况下 atom 在、agent 不在：
//!
//! 1. spawn 那一轮被 `undo_turn` 撤了（applier 只写值不毁 atom——019）；
//! 2. despawn 之后留下的那个墓碑（`ToolsAllowed` 槽位刻意不逐出，见
//!    `despawn.rs` 的判断）；
//! 3. 019 的按需重建在 undo 路径上凭键建出来的空壳。
//!
//! 三种情况**在状态上完全一致**，因为它们本来就是同一种状态：这个 agent 没有
//! 「被 spawn 出来、带着一份工具子集」这个事实。把「活着」定义成图上的一个值
//! （而不是「atom 在不在」），undo 撤销一次 spawn 就退化成一次普通的值回滚，
//! 跟别的 primitive 一视同仁——这是 028 那条 undo 语义裁决的落点。
//!
//! root 是唯一的例外：**它的活性就是会话本身的存在**，不来自任何一次 spawn，
//! 所以它的 `ToolsAllowed` 是 `Null` 而它活着。

use std::collections::BTreeSet;

use crate::graph::{AtomKey, Slot};
use crate::ids::AgentId;
use crate::value::atom_value::AgentValue;

use super::session::Session;

impl Session {
    /// 这个 id 是不是本会话这棵树上的（root 自己算）。
    ///
    /// 跨会话不共享 store（STATE-MODEL §「并发」：一个 root agent + 它的整棵子树
    /// = 一个 session = 一个 actor 线程 = 一个 store），所以别的树上的 id 在这里
    /// 一律不认——这是 spawn / despawn / 跨 agent 读三处的第一道校验。
    pub fn in_session(&self, agent: &AgentId) -> bool {
        agent == &self.agent || self.agent.is_ancestor_of(agent)
    }

    /// 这个 agent 现在活着吗（见模块文档的定义）。
    ///
    /// 用**非创建**查找：探一个不存在的 id 不该在 family 里留下十个 atom。
    /// 读口用 get-or-create 的话，宿主传错一个 id 就是一次静默的图污染。
    pub fn is_live(&self, agent: &AgentId) -> bool {
        if agent == &self.agent {
            return true;
        }
        if !self.in_session(agent) {
            return false;
        }
        !matches!(
            self.peek(&AtomKey::Agent(agent.clone(), Slot::ToolsAllowed)),
            None | Some(AgentValue::Null)
        )
    }

    /// 直接子 agent 里**还活着的**那些，按 id 排序（`AgentId` 的 `Ord` 是字典序，
    /// 稳定即可——这个列表会进 UI 时间线，顺序不定的列表每次刷新都在跳）。
    ///
    /// 复杂度 O(family 键数)。**刻意接受**（STATE-MODEL §「汇聚 atom 的复杂度」
    /// 要求写这类东西时明确选一个）：一个会话的 agent 数被深度 ≤3 / 子数 ≤8 结构性
    /// 地限住，而维护一份增量的「子列表」意味着又一个要跟 undo 对齐的计数器——
    /// 那正是本仓最恨的那类 bug 的形状。
    pub fn children_of(&self, parent: &AgentId) -> Vec<AgentId> {
        let mut out: Vec<AgentId> = self
            .known_agents()
            .into_iter()
            .filter(|a| a.parent().as_ref() == Some(parent) && self.is_live(a))
            .collect();
        out.sort();
        out
    }

    /// 会话里所有**还活着的** agent，root 在最前（root 的 id 是最短的前缀，
    /// 字典序天然把它排在自己的后代之前）。
    pub fn live_agents(&self) -> Vec<AgentId> {
        let mut out: Vec<AgentId> = self
            .known_agents()
            .into_iter()
            .filter(|a| self.is_live(a))
            .collect();
        out.sort();
        out
    }

    /// 以 `root` 为根的活子树（含 `root` 自己），**自叶向根**排序。
    ///
    /// 这个顺序就是 019 硬约束 1 要的那个：先销下游（derived）后销上游
    /// （primitive）、子树递归。深的排前面，同深度按 id 倒序——完全确定，
    /// despawn 的报告因此可以被逐条断言。
    pub(super) fn live_subtree_leaf_first(&self, root: &AgentId) -> Vec<AgentId> {
        let mut out: Vec<AgentId> = self
            .known_agents()
            .into_iter()
            .filter(|a| (a == root || root.is_ancestor_of(a)) && self.is_live(a))
            .collect();
        out.sort_by(|a, b| b.depth().cmp(&a.depth()).then_with(|| b.cmp(a)));
        out
    }

    /// family 键空间里出现过的所有 agent——**活的和墓碑都在里面**。
    ///
    /// spawn 铸号要的正是这一份（见 `spawn.rs`：号从这里取最大值往上走，
    /// 于是一个 id 只属于一个 agent 的一生）。
    pub(super) fn known_agents(&self) -> BTreeSet<AgentId> {
        self.sources
            .borrow()
            .iter()
            .map(|(key, _)| key.agent().clone())
            .collect()
    }

    /// 非创建读：键不在 family 里就是 `None`，**不会顺手建一个**。
    ///
    /// 跨 agent 读口（`cross_read.rs`）和活性判定都走它。命令层写入走的是
    /// `graph::source_atom`（get-or-create），两条路径的差别是刻意的：写入必须
    /// 保证目标存在，读取不该有副作用。
    pub(super) fn peek(&self, key: &AtomKey) -> Option<AgentValue> {
        let id = self.sources.borrow().get(key)?;
        Some(self.store.get(id))
    }
}
