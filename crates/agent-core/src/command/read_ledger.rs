//! [`Session`] 的另一半读口：**整份状态与整条日志**。
//!
//! 隔壁 [`read`](super::read) 是**取料口**——「这个 agent 此刻某个槽位是什么」，
//! 一次读一格，宿主拿去组 `Ingredients`。这个文件反过来，读的都是**跨槽位、
//! 跨 agent 的整体**：一份完整快照、一条完整日志、一个诊断计数。两者的读者也
//! 不同——取料口的读者是 adapter 与工具层，这里的读者是持久化（011）、恢复
//! （027）、审计与测试。
//!
//! 208 把它拆出来（红线 9：`read.rs` 顶破 300 行）。拆的判据不是行数：一句话
//! 说得清各自是干嘛的，且两句都不含「和」——**「读一个 agent 的一格」**与
//! **「读整份状态」**。

use crate::graph::AtomKey;
use crate::value::atom_value::AgentValue;

use super::meta::{AgentEntry, AgentHistory};
use super::session::Session;

impl Session {
    /// **完整状态**：所有 primitive 的当前值，按逻辑键排序。
    ///
    /// 这就是 010 的 `Snapshot` 形状（`Vec<(AtomKey, Value)>`，只存 primitive）。
    /// 排序不是装饰：两份快照要能逐值比对（「undo 一整 turn 后所有 primitive 逐值
    /// 回退」是 M2 验收的核心句），顺序不定的快照比不出来。
    ///
    /// derived 一个都不在里面，也进不来——它们的键是 `DerivedKey`，另一张表
    /// （`graph::slot` 的裁决）。
    pub fn primitives(&self) -> Vec<(AtomKey, AgentValue)> {
        let ids: Vec<(AtomKey, agent_store::AtomId)> = self
            .sources
            .borrow()
            .iter()
            .map(|(key, id)| (key.clone(), id))
            .collect();
        let mut out: Vec<(AtomKey, AgentValue)> = ids
            .into_iter()
            .map(|(key, id)| (key, self.store.get(id)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// 只读的 command log。011 的持久化从这里读整份日志，测试从这里数条目。
    ///
    /// 是 `&` 不是 `&mut`：日志的写入口只有命令（`step` / `begin_turn` / …），
    /// 借出可变引用等于给「手写一条 entry」开了门。
    pub fn history(&self) -> &AgentHistory {
        &self.history
    }

    /// 日志条数（含被 undo 掉、还能 redo 回来的尾巴）。
    pub fn history_len(&self) -> usize {
        self.history.len()
    }

    /// 游标 = 已生效条数。`history_len() - cursor()` 就是能 redo 回来的条数。
    pub fn cursor(&self) -> usize {
        self.history.cursor()
    }

    /// 最后一条 entry（物理最后，不一定是 undo 要弹的那一条）。测试与审计用。
    pub fn last_entry(&self) -> Option<&AgentEntry> {
        self.history.last()
    }

    /// 诊断探针：derived 到目前为止真的重算了多少次。
    ///
    /// 存在的理由是「undo 之后 derived **重算**一致」和「停在旧值碰巧也一致」在
    /// 断言上长得一模一样，只有这个计数分得开。跟 `agent-store` 的
    /// `debug_recompute_count` 一样是 `#[doc(hidden)]`——它不是公开面的一部分。
    #[doc(hidden)]
    pub fn debug_recompute_count(&self) -> usize {
        self.store.debug_recompute_count()
    }
}
