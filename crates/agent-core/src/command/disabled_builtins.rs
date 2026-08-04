//! [`Session::disable_builtins`] / [`Session::disabled_builtins`]：这个会话关掉了
//! 哪些内置工具的 journaled 读写（076，形状照 [`host_tools`](super::host_tools) 与
//! [`host_skills`](super::host_skills) 的既有先例）。
//!
//! ## 它是**减法**，跟隔壁三个槽位方向相反
//!
//! `HostTools` / `HostSkills` 记的是「宿主往这个会话里加了什么」；这一个记的是
//! 「这个会话把部署方给的哪几件**藏起来不给模型看**」。「不启用」在这里的定义只有
//! 一个：**连名字带描述都不进 prompt**，模型压根不知道有它——不是「看得见但不给调」，
//! 也不是「预先激活正文」。
//!
//! **只能减不能加**：列表里的名字必须在部署方装配出来的那张表里，这条闸在
//! `agent-server` 的 HTTP 路由上（作者还在场的那一刻），不在这里。core 只负责如实
//! 记下这个会话当初关了哪些名字——它是一份历史，不是一次请求的校验。
//!
//! ## 为什么它也必须进 store（073 那三条原样成立）
//!
//! 1. **历史对话是在那一份减过的表下产生的。** 一个关掉了 `srv:shell/exec` 的会话，
//!    模型整段历史里都没见过这个工具；恢复时按今天的开关重建，模型会突然多出一件
//!    它从没被告知过的能力，而历史里没有任何铺垫。
//! 2. **红线 11。** 工具表在 prompt 最前面。开关不落盘 = 恢复出来的第一轮表就变了
//!    = 前缀全断，而恢复出来的会话本该接着用缓存。
//! 3. **恢复是忠实重放，不是用今天的配置重建。** 把 per-session 的开关当部署配置，
//!    等于在 undo / redo / 崩溃恢复 / 审计这套投影里开一个洞。
//!
//! 反过来说也一样：**已有历史的会话再带这个字段 → 400 `session_has_history`**，
//! 跟 073 完全同一条闸，不是新错误码。
//!
//! ## 只在建会话时写一次（不做运行时增删）
//!
//! 命令本身是幂等的覆盖写，但**调用点只有一处**：宿主开这个会话的 actor 时，且只在
//! 「这是一个全新会话」那一支。会话中途改工具表 = 前缀缓存那一刻全断，
//! `docs/HOST-CAPABILITIES.md` §三 明确不做。
//!
//! ## 落值前排序去重（红线 11）
//!
//! 走 `value::str_set`——跟 spawn 的工具子集（`ToolsAllowed`）、039 的激活集
//! （`SkillsActive`）同一处编解码。它是「一组字符串」这个形状的第三个用户，而排序
//! 去重这一步**不能在三个地方各写一遍**（写漏一处就是那一处每轮全价且不报错）。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::value::str_set;

use super::session::Session;

impl Session {
    /// 记下这个会话关掉了哪些内置工具：一条 `Entry`（label `"disable_builtins"`）。
    ///
    /// 落在 **root** 头上（会话级命令，同 [`Session::declare_host_tools`]）——开关
    /// 属于这个会话，不属于树上某一个 agent，整棵 agent 树共用它（子 agent 不单独
    /// 配，076 用户拍板）。值经 [`str_set::to_value`] **排序去重**再落盘（红线 11）。
    ///
    /// 传空 `Vec` 是一次真正的空操作：值跟默认值相等，`record_set` 不产生 `Change`，
    /// `History::append` 不落条目——「什么都没关的会话」因此连一条幽灵 entry 都没有，
    /// 它的会话文件跟 076 之前逐字节相同。
    pub fn disable_builtins(&mut self, names: Vec<Arc<str>>) {
        let key = self.disabled_builtins_key();
        let value = str_set::to_value(names);
        self.commit("disable_builtins", |txn| txn.set_key(key, value));
    }

    /// 这个会话关掉的内置工具名（**排序去重**——写入时排过，读回就是有序的）。
    ///
    /// 宿主（`agent-server` 的 actor）在装配 `ToolTable` 时读它，把这些名字从部署期
    /// 那五档里整条剔掉。空 = 一个都没关，工具表跟 076 之前逐字节相同。
    pub fn disabled_builtins(&self) -> Vec<Arc<str>> {
        let key = self.disabled_builtins_key();
        // 非创建读（`peek`，同 `host_tools`）：这个槽位在 `build_agent` 建图时就有
        // 默认值，兜底那一支只在「宿主拼错了 root id」时走得到。
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        str_set::from_value(&value)
    }

    /// 开关只落在 root 头上（会话级）。两个口共用一处，免得一处写 root、另一处写
    /// 别的 agent，读写从此对不上——那正是静默错值的长相。
    fn disabled_builtins_key(&self) -> AtomKey {
        AtomKey::Agent(self.agent.clone(), Slot::DisabledBuiltins)
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::AgentId;

    use super::*;

    fn names(session: &Session) -> Vec<String> {
        session.disabled_builtins().iter().map(|n| n.to_string()).collect()
    }

    /// 关掉 → 读回（排序去重）；这是**一条 journaled entry**，undo 一下工具就回来了
    /// ——「undo 白拿」在 core 这一层的落点。
    #[test]
    fn a_switch_is_journaled_and_undo_takes_it_back() {
        let mut s = Session::new(AgentId::root());
        assert!(s.disabled_builtins().is_empty(), "全新会话一个内置工具都没关");

        let before = s.history_len();
        s.disable_builtins(vec![
            Arc::from("srv:shell/exec"),
            Arc::from("srv:agent/spawn"),
            Arc::from("srv:shell/exec"),
        ]);
        assert_eq!(s.history_len(), before + 1, "开关是一条 journaled entry，不是一个不进日志的构造参数");
        assert_eq!(names(&s), vec!["srv:agent/spawn", "srv:shell/exec"], "排序去重（红线 11）");

        let report = s.undo_step();
        assert!(matches!(report, crate::command::UndoReport::Applied { .. }), "{report:?}");
        assert!(s.disabled_builtins().is_empty(), "undo 越过开关那一步之后，这个会话不该还关着任何东西");

        let _ = s.redo_step();
        assert_eq!(names(&s), vec!["srv:agent/spawn", "srv:shell/exec"]);
    }

    /// 空开关不落 entry：不带这个字段的会话，日志跟 076 之前一个字节不差。
    #[test]
    fn disabling_nothing_leaves_no_trace() {
        let mut s = Session::new(AgentId::root());
        let before = s.history_len();
        s.disable_builtins(Vec::new());
        assert_eq!(s.history_len(), before, "空开关不该留下一条幽灵 entry");
        assert!(s.disabled_builtins().is_empty());
    }
}
