//! [`Session::declare_host_tools`] / [`Session::host_tools`]：宿主注入的工具声明的
//! journaled 读写（073）。
//!
//! 用户 2026-08-04 拍板：
//!
//! > 历史对话记录，不用对工具再注入一次。**历史对话就该跟历史一致，原模原样 100% 复刻。**
//!
//! ## 为什么是 command 而不是「开会话时的一个参数」（红线 2/3）
//!
//! 注入的声明是**会话状态，不是部署配置**。走 `commit` 写 `Slot::HostTools` 会留下
//! 一条 `Entry`，于是三件事一次白拿，都不需要任何新机制：
//!
//! - **崩溃恢复**：跟别的 primitive 一样从日志回放自动回来，宿主重连时**不必也不该**
//!   再声明一遍——历史对话是在**那一份**工具表下产生的，用今天的新清单重建就自相
//!   矛盾（模型当初说「我调了 `web:crm/lookup`」，而今天的清单里可能没有它了）；
//! - **红线 11**：工具表在 prompt 最前面，恢复时换一份 = 第一轮前缀全断，而恢复
//!   出来的会话本该接着用缓存；
//! - **undo**：声明发生在会话建立那一步，`undo` 到它之前，槽位退回空数组——跟别的
//!   primitive 一视同仁，退化成一次普通的值回滚。
//!
//! 这跟 skill 的既有模式（[`skill`](super::skill)）同构：激活状态在 store、内容在
//! 运行时 registry；注入的能力是**声明**在 store、**执行**在宿主侧。不是新发明。
//!
//! ## 只在建会话时写一次（不做运行时增删）
//!
//! 命令本身是幂等的覆盖写（同 `set_max_turns`），但**调用点只有一处**：宿主开这个
//! 会话的 actor 时，且只在「这是一个全新会话」那一支。会话中途换工具表 = 前缀缓存
//! 那一刻全断，`docs/HOST-CAPABILITIES.md` §三 明确不做。

use crate::graph::{AtomKey, Slot};
use crate::value::host_tools;
use crate::value::tool::{Reversibility, ToolSpec};

use super::session::Session;

impl Session {
    /// 记下宿主为这个会话声明的工具：一条 `Entry`（label `"declare_host_tools"`）。
    ///
    /// 落在 **root** 头上（会话级命令，同 `set_max_turns`）——声明属于这个会话，
    /// 不属于树上某一个 agent。值经 [`host_tools::to_value`] **按名字排序**再落盘
    /// （红线 11，理由见那个模块）。
    ///
    /// 传空 `Vec` 是一次真正的空操作：值跟默认值相等，`record_set` 不产生 `Change`，
    /// `History::append` 不落条目——「没声明的会话」因此连一条幽灵 entry 都没有，
    /// 它的会话文件跟 073 之前逐字节相同。
    pub fn declare_host_tools(&mut self, tools: Vec<(ToolSpec, Reversibility)>) {
        let key = self.host_tools_key();
        let value = host_tools::to_value(tools);
        self.commit("declare_host_tools", |txn| txn.set_key(key, value));
    }

    /// 这个会话被声明过的工具（**按名字排序**——写入时排过，读回就是有序的）。
    ///
    /// 宿主（`agent-server` 的 actor）在装配 `ToolTable` 时读它。空 = 没有任何注入，
    /// 工具表跟不带声明的会话逐字节相同。
    pub fn host_tools(&self) -> Vec<(ToolSpec, Reversibility)> {
        let key = self.host_tools_key();
        // 非创建读（`peek`，同 `active_skill_names`）：这个槽位在 `build_agent` 建图
        // 时就有默认值，兜底那一支只在「宿主拼错了 root id」时走得到。
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        host_tools::from_value(&value)
    }

    /// 声明只落在 root 头上（会话级）。两个口共用一处，免得一处写 root、另一处
    /// 写别的 agent，读写从此对不上——那正是静默错值的长相。
    fn host_tools_key(&self) -> AtomKey {
        AtomKey::Agent(self.agent.clone(), Slot::HostTools)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ids::AgentId;

    use super::*;

    fn tool(name: &str, reversibility: Reversibility) -> (ToolSpec, Reversibility) {
        let spec = ToolSpec {
            name: Arc::from(name),
            description: Arc::from("说明"),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        };
        (spec, reversibility)
    }

    /// 声明 → 读回（排序）；这是**一条 journaled entry**，undo 一下就回到没有注入
    /// 的状态——「undo 白拿」在 core 这一层的落点。
    #[test]
    fn a_declaration_is_journaled_and_undo_takes_it_back() {
        let mut s = Session::new(AgentId::root());
        assert!(s.host_tools().is_empty(), "全新会话没有任何注入");

        let before = s.history_len();
        s.declare_host_tools(vec![
            tool("web:crm/lookup", Reversibility::Pure),
            tool("desk:clipboard/write", Reversibility::Irreversible),
        ]);
        assert_eq!(
            s.history_len(),
            before + 1,
            "声明是一条 journaled entry，不是一个不进日志的构造参数"
        );

        let names: Vec<String> = s
            .host_tools()
            .iter()
            .map(|(spec, _)| spec.name.to_string())
            .collect();
        assert_eq!(names, vec!["desk:clipboard/write", "web:crm/lookup"]);
        assert_eq!(s.host_tools()[1].1, Reversibility::Pure);

        // undo 到声明之前：工具表回到没有注入的状态（白拿的那一条）。
        let report = s.undo_step();
        assert!(
            matches!(report, crate::command::UndoReport::Applied { .. }),
            "{report:?}"
        );
        assert!(
            s.host_tools().is_empty(),
            "undo 越过声明那一步之后，这个会话不该还认得那些工具"
        );

        // redo 追回来——跟别的 primitive 一视同仁。
        let _ = s.redo_step();
        assert_eq!(s.host_tools().len(), 2);
    }

    /// 空声明不落 entry：不带 `capabilities` 的会话，日志跟 073 之前一个字节不差。
    #[test]
    fn declaring_nothing_leaves_no_trace() {
        let mut s = Session::new(AgentId::root());
        let before = s.history_len();
        s.declare_host_tools(Vec::new());
        assert_eq!(s.history_len(), before, "空声明不该留下一条幽灵 entry");
        assert!(s.host_tools().is_empty());
    }
}
