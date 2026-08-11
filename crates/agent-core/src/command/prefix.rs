//! [`Session::set_prefix_chunks`] / [`Session::prefix_chunks`]：会话创建期定下的那
//! 一列 system 前缀块的 journaled 读写（134，形状照
//! [`host_skills`](super::host_skills) 的既有先例）。
//!
//! ## core 在这里知道什么、不知道什么
//!
//! 知道的：**这个会话的 system 段前面挂着一列带 label 的文本块，创建期定下，之后
//! 不变**。不知道的：这些文本是哪来的（135 那些"开局工具"跑出来的结果、一份配置、
//! 还是宿主手填）。红线 12 的精神——core 里不该出现「时机」「skill」这类词，
//! 它看见的就是一列文本块，跟看见 `Slot::HostTools` 是一列声明一样。
//!
//! ## 为什么走 command 而不是构造参数（红线 2/3）
//!
//! 跟 073/064 给宿主声明定下的是同一条：**创建期算出来的东西是会话状态，不是部署
//! 配置**。走 `commit` 写 `Slot::PrefixChunks` 留下一条 `Entry`，崩溃恢复 / undo /
//! 审计回放因此白拿——恢复时**不重算**，值就是状态（这正是 135「重启不重跑开局
//! 工具」那条验收在 core 侧的前提）。
//!
//! 做成构造参数的那一版会长这样：恢复出来的会话得再跑一遍开局工具才有前缀，而
//! 那些工具这一次给出的结果跟当初**不保证**一样（它们读的是外部世界）。于是历史
//! 里的对话是在 A 前缀下产生的、恢复出来的会话挂着 B 前缀——前缀在 prompt 最前面，
//! 缓存当场全断，模型的上下文也跟历史对不上。两样都不报错。
//!
//! ## undo 语义：一条普通的 journaled entry，没有第二套规矩
//!
//! 逐字对齐 `declare_host_skills`：**entry 级 undo 退得掉、redo 回得来**。用户面的
//! turn 级 undo 永远走不到它——它写在第一轮之前，不属于任何 turn。不要因此发明
//! 「不可 undo 的状态」：单一线性日志（决策 4）里没有这个概念，造一个出来就等于
//! 在日志上开了一个 undo 到不了的洞。
//!
//! ## 只在建会话时写一次
//!
//! 命令本身是幂等的覆盖写，但调用点只有一处：宿主开这个会话时、且只在「这是一个
//! 全新会话」那一支。会话中途换前缀 = 前缀缓存那一刻全断，跟
//! `docs/HOST-CAPABILITIES.md` §三 对 skill 索引的裁决同一条。

use crate::graph::{AtomKey, Slot};
use crate::seam::SystemChunk;
use crate::value::prefix_chunks;

use super::session::Session;

impl Session {
    /// 记下这个会话的 system 前缀块：一条 `Entry`（label `"prefix_init"`）。
    ///
    /// 落在 **root** 头上（会话级命令，同 [`Session::declare_host_skills`]）——
    /// 前缀属于这个会话，不属于树上某一个 agent。值经 `prefix_chunks::to_value`
    /// **原样顺序**落盘（不排序，理由见 `value::prefix_chunks` 的模块文档：
    /// 顺序本身是信息，红线 11 要的确定性由「一次写定」这个写入点保证）。
    ///
    /// 传空 `Vec` 是一次真正的空操作：值跟默认值相等，`record_set` 不产生 `Change`，
    /// `History::append` 不落条目——「没有前缀块的会话」因此连一条幽灵 entry 都没有，
    /// 日志跟 134 之前逐字节相同。
    pub fn set_prefix_chunks(&mut self, chunks: Vec<SystemChunk>) {
        let key = self.prefix_chunks_key();
        let value = prefix_chunks::to_value(&chunks);
        self.commit("prefix_init", |txn| txn.set_key(key, value));
    }

    /// 这个会话的 system 前缀块，**顺序 = 写入顺序**。组料方（135）读它。
    ///
    /// 空 = 这个会话没有前缀块，system 段跟 134 之前逐字节相同。
    pub fn prefix_chunks(&self) -> Vec<SystemChunk> {
        let key = self.prefix_chunks_key();
        // 非创建读（`peek`，同 `host_skills`）：这个槽位在 `build_agent` 建图时就有
        // 默认值，兜底那一支只在「宿主拼错了 root id」时走得到。
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        prefix_chunks::from_value(&value)
    }

    /// 前缀只落在 root 头上（会话级）。两个口共用一处，免得一处写 root、另一处写
    /// 别的 agent，读写从此对不上——那正是静默错值的长相。
    fn prefix_chunks_key(&self) -> AtomKey {
        AtomKey::Agent(self.agent.clone(), Slot::PrefixChunks)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ids::AgentId;

    use super::*;

    fn chunk(label: &str, text: &str) -> SystemChunk {
        SystemChunk {
            label: Arc::from(label),
            text: Arc::from(text),
        }
    }

    /// 写入 → 读回：逐字节相同、**顺序 = 写入顺序**（不是按 label 排的）。
    /// 这是一条 journaled entry，label 是 `"prefix_init"`。
    #[test]
    fn the_written_chunks_come_back_in_order_as_one_journaled_entry() {
        let mut s = Session::new(AgentId::root());
        assert!(s.prefix_chunks().is_empty(), "全新会话没有任何前缀块");

        let before = s.history_len();
        let written = vec![chunk("zeta", "后一块"), chunk("alpha", "前一块")];
        s.set_prefix_chunks(written.clone());

        assert_eq!(
            s.history_len(),
            before + 1,
            "写入是一条 journaled entry，不是一个不进日志的构造参数"
        );
        assert_eq!(
            s.last_entry().expect("刚写过").meta.label,
            "prefix_init",
            "label 要回答「当时发生了什么」"
        );
        assert_eq!(
            s.prefix_chunks(),
            written,
            "顺序 = 写入顺序，两个字段逐字节相同"
        );
    }

    /// entry 级 undo 退得掉、redo 回得来——逐字对齐
    /// `declare_host_skills` 的先例，**不发明「不可 undo 的状态」**。
    #[test]
    fn undo_takes_the_prefix_back_and_redo_brings_it_again() {
        let mut s = Session::new(AgentId::root());
        s.set_prefix_chunks(vec![chunk("alpha", "一"), chunk("beta", "二")]);

        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert!(
            s.prefix_chunks().is_empty(),
            "undo 越过这一步之后，这个会话不该还挂着那份前缀"
        );

        let _ = s.redo_step();
        let back = s.prefix_chunks();
        assert_eq!(back.len(), 2);
        assert_eq!(&*back[0].label, "alpha", "redo 回来的顺序也得是原来那个");
        assert_eq!(&*back[1].text, "二");
    }

    /// 空写入不落 entry：没有前缀块的会话，日志跟 134 之前一个字节不差。
    #[test]
    fn writing_nothing_leaves_no_trace() {
        let mut s = Session::new(AgentId::root());
        let before = s.history_len();
        s.set_prefix_chunks(Vec::new());
        assert_eq!(s.history_len(), before, "空写入不该留下一条幽灵 entry");
        assert!(s.prefix_chunks().is_empty());
    }
}
