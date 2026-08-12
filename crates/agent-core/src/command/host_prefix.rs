//! [`Session::declare_host_prefix`] / [`Session::host_prefix`]：宿主经
//! `capabilities.prefix` 声明的开局块的 journaled 读写（154，决策 31 的状态位，
//! 形状照 [`host_tools`](super::host_tools) 的既有先例 1:1 镜像）。
//!
//! ## 为什么是 command 而不是「开会话时的一个参数」（红线 2/3）
//!
//! 跟 073 给宿主注入的工具定下的是同一条：**声明是会话状态，不是部署配置**。
//! 走 `commit` 写 `Slot::HostPrefix` 会留下一条 `Entry`，于是三件事一次白拿，
//! 都不需要任何新机制：
//!
//! - **崩溃恢复**：跟别的 primitive 一样从日志回放自动回来，宿主重连时**不必也
//!   不该**再声明一遍——历史对话是在**那一份**开局块下产生的，用今天的新声明
//!   重建就自相矛盾；
//! - **红线 11**：开局块排在 system 段最前面，恢复时换一份 = 第一轮前缀全断，
//!   而恢复出来的会话本该接着用缓存；
//! - **undo**：声明发生在会话建立那一步，`undo` 到它之前，槽位退回空数组——跟别的
//!   primitive 一视同仁，退化成一次普通的值回滚。
//!
//! ## 只在建会话时写一次（不做运行时增删）
//!
//! 命令本身是幂等的覆盖写（同 `declare_host_tools`），但**调用点只有一处**：宿主
//! 开这个会话的 actor 时，且只在「这是一个全新会话」那一支。会话中途换开局块 =
//! 前缀缓存那一刻全断，跟 `docs/HOST-CAPABILITIES.md` §三 对声明的裁决同一条。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::value::host_prefix;

use super::session::Session;

impl Session {
    /// 记下宿主为这个会话声明的开局块：一条 `Entry`（label
    /// `"declare_host_prefix"`）。
    ///
    /// 落在 **root** 头上（会话级命令，同 `Session::declare_host_tools`）——声明
    /// 属于这个会话，不属于树上某一个 agent。值经 [`host_prefix::to_value`]
    /// **按 name 排序**再落盘（红线 11，理由见那个模块）。
    ///
    /// 传空 `Vec` 是一次真正的空操作：值跟默认值相等，`record_set` 不产生
    /// `Change`，`History::append` 不落条目——「没声明开局块的会话」因此连一条
    /// 幽灵 entry 都没有，它的会话文件跟 154 之前逐字节相同。
    pub fn declare_host_prefix(&mut self, prefix: Vec<(Arc<str>, Arc<str>)>) {
        let key = self.host_prefix_key();
        let value = host_prefix::to_value(prefix);
        self.commit("declare_host_prefix", |txn| txn.set_key(key, value));
    }

    /// 这个会话被声明过的开局块（**按 name 排序**——写入时排过，读回就是有序的）。
    ///
    /// **这不是拼进 system 段的那份文本**——那是 [`Slot::PrefixChunks`]（134），
    /// 由 155 的合成 timed 工具经 `run_session_start` 实际落块。这个读口给的是
    /// **原始声明本身**：155/156 的装配/HTTP 层用它做恢复期判定（这个会话是不是
    /// 已经声明过、声明的是不是这一份），跟 073 的 `host_tools()` 同一个定位。
    /// 空 = 没有任何声明。
    pub fn host_prefix(&self) -> Vec<(Arc<str>, Arc<str>)> {
        let key = self.host_prefix_key();
        // 非创建读（`peek`，同 `host_tools`）：这个槽位在 `build_agent` 建图时就有
        // 默认值，兜底那一支只在「宿主拼错了 root id」时走得到。
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        host_prefix::from_value(&value)
    }

    /// 声明只落在 root 头上（会话级）。两个口共用一处，免得一处写 root、另一处
    /// 写别的 agent，读写从此对不上——那正是静默错值的长相。
    fn host_prefix_key(&self) -> AtomKey {
        AtomKey::Agent(self.agent.clone(), Slot::HostPrefix)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ids::AgentId;

    use super::*;

    fn pair(name: &str, text: &str) -> (Arc<str>, Arc<str>) {
        (Arc::from(name), Arc::from(text))
    }

    /// 声明 → 读回（排序）；这是**一条 journaled entry**，undo 一下就回到没有声明
    /// 的状态——「undo 白拿」在 core 这一层的落点。
    #[test]
    fn a_declaration_is_journaled_and_undo_takes_it_back() {
        let mut s = Session::new(AgentId::root());
        assert!(s.host_prefix().is_empty(), "全新会话没有任何声明");

        let before = s.history_len();
        s.declare_host_prefix(vec![
            pair("zeta", "后声明的"),
            pair("alpha", "先声明的"),
        ]);
        assert_eq!(
            s.history_len(),
            before + 1,
            "声明是一条 journaled entry，不是一个不进日志的构造参数"
        );
        assert_eq!(
            s.last_entry().expect("刚写过").meta.label,
            "declare_host_prefix",
            "label 要回答「当时发生了什么」"
        );

        let names: Vec<String> = s
            .host_prefix()
            .iter()
            .map(|(name, _)| name.to_string())
            .collect();
        assert_eq!(names, vec!["alpha", "zeta"], "读回按 name 排过序");
        assert_eq!(&*s.host_prefix()[0].1, "先声明的");

        // undo 到声明之前：开局块回到没有声明的状态（白拿的那一条）。
        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert!(
            s.host_prefix().is_empty(),
            "undo 越过声明那一步之后，这个会话不该还认得那些开局块"
        );

        // redo 追回来——跟别的 primitive 一视同仁。
        let _ = s.redo_step();
        assert_eq!(s.host_prefix().len(), 2);
    }

    /// 空声明不落 entry：不带 `capabilities.prefix` 的会话，日志跟 154 之前一个
    /// 字节不差。
    #[test]
    fn declaring_nothing_leaves_no_trace() {
        let mut s = Session::new(AgentId::root());
        let before = s.history_len();
        s.declare_host_prefix(Vec::new());
        assert_eq!(s.history_len(), before, "空声明不该留下一条幽灵 entry");
        assert!(s.host_prefix().is_empty());
    }
}
