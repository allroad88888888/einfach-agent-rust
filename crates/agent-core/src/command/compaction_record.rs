//! [`Session::summary_library`]：压缩可见性的读口（109）。
//!
//! 109 的接线约束 1（issue 原文）：**展开原文要走完整记录那条链，不经过
//! [`crate::value::send_plan::project`]**——`project` 回答的是「这一轮发什么」，
//! 展开压缩点要的是「有什么」。这个文件只补一半：摘要正文那一半（另一半，
//! 完整消息历史，`Session::messages_of` 本来就有，见 [`super::read`]）。
//!
//! 摘要正文从 `Slot::Summaries` 取（接线约束 5）——那个生成摘要的子 agent
//! 早被 108 回收了，不能从它那边反推。
//!
//! 跟 [`super::apply_summary`] 内部那个同名读法是**同一份底层数据、两个入口**：
//! 那边的私有 `summaries_of` 只服务 `apply_summary` 自己的幂等判定，不对外；
//! 这里独立开一个 `pub` 方法，不是给它提权——`apply_summary.rs` 贴着行数天花板
//! （298 行），新增的读口另起一个文件（`docs/WORKFLOW.md` 的一贯做法）。两处
//! 各自 `peek` 同一个槽位、调同一个 [`summaries::from_value`]，没有第二套解码
//! 逻辑，只是多了一处读它的入口。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::{AgentId, SummaryId};
use crate::value::summaries;

use super::session::Session;

impl Session {
    /// 这个 agent 迄今全部压缩摘要的正文，**插入顺序 = 压缩发生的顺序**
    /// （107：`Slot::Summaries` 只增不删，追加顺序由写入点保证确定，红线 1）。
    ///
    /// 非创建读（`peek`，同 [`Session::send_plan_of`] 的先例）：探一个不在树上
    /// 的 id 不该在 family 里留下一个没人写的 atom。找不到就是空库（还没压缩
    /// 过），不是错误。
    pub fn summary_library(&self, agent: &AgentId) -> Vec<(SummaryId, Arc<str>)> {
        let key = AtomKey::Agent(agent.clone(), Slot::Summaries);
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        summaries::from_value(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(AgentId::root())
    }

    /// 没压缩过就是空库，不是 panic 或者错误。
    #[test]
    fn a_fresh_agent_has_an_empty_library() {
        let s = session();
        assert!(s.summary_library(&AgentId::root()).is_empty());
    }

    /// 两次压缩之后，库里两条，插入顺序 = 压缩发生的顺序——跟
    /// `Session::apply_summary` 走的是同一份底层数据，不是另一套账。
    #[test]
    fn it_reflects_apply_summary_in_insertion_order() {
        let mut s = session();
        let root = AgentId::root();
        s.apply_summary(&root, 3, Arc::from("第一份")).unwrap();
        s.apply_summary(&root, 8, Arc::from("第二份")).unwrap();

        let library = s.summary_library(&root);
        assert_eq!(library.len(), 2);
        assert_eq!(library[0].1.as_ref(), "第一份");
        assert_eq!(library[1].1.as_ref(), "第二份");
    }

    /// `/undo` 一次，最近一次压缩的摘要跟着从库里退回——压缩可见性白拿这条
    /// 一致性，不是额外维护出来的（109 接线约束 2 的地基）。
    #[test]
    fn undo_takes_the_summary_back_out_of_the_library() {
        let mut s = session();
        let root = AgentId::root();
        s.apply_summary(&root, 5, Arc::from("摘要")).unwrap();
        assert_eq!(s.summary_library(&root).len(), 1);

        let report = s.undo_step();
        assert!(matches!(
            report,
            crate::command::UndoReport::Applied { .. }
        ));
        assert!(s.summary_library(&root).is_empty());
    }
}
