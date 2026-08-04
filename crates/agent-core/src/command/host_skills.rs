//! [`Session::declare_host_skills`] / [`Session::host_skills`]：宿主注入的 skill
//! 声明的 journaled 读写（064，形状照 [`host_tools`](super::host_tools) 的既有先例）。
//!
//! ## 为什么 skill 声明也走 command（红线 2/3）
//!
//! 跟 073 给注入的工具定下的是同一条：**注入的声明是会话状态，不是部署配置**。
//! 走 `commit` 写 `Slot::HostSkills` 会留下一条 `Entry`，崩溃恢复 / undo 因此白拿。
//!
//! 但 skill 这一路比工具那一路**更不能不存**，多两条理由：
//!
//! 1. **激活集早就在 store 里了**（[`Slot::SkillsActive`](crate::graph::Slot::SkillsActive)，
//!    039）。声明不落盘 = 恢复出来的会话有一份指向空 registry 的激活集：状态说
//!    `crm-flow` 激活着，展开注入却什么都取不到（查不到的 id 静默跳过），而模型的
//!    历史里明明写着它读过那段正文。**悬空引用 + 静默**，正是本仓最怕的形状。
//! 2. **宿主没有第二次机会报**：073 之后，有历史的会话再带 `capabilities` 一律 400
//!    `session_has_history`。不存 = 永久没了，连「重连时重报一遍」这条（已被否决的）
//!    退路都不存在。
//!
//! ## 跟 [`skill`](super::skill) 的分工：激活集 vs. 内容
//!
//! `SkillsActive` 记的是**哪些 id 被激活**，内容从运行时 `SkillRegistry` 现取；
//! 本模块记的是**宿主这一次报进来的内容本身**。两者不重复：磁盘装载的 skill 内容
//! 在本机文件里（store 外有第二份），宿主注入的**只在那一次 HTTP 请求里存在过**
//! ——store 外没有第二处可取。
//!
//! ## 只在建会话时写一次（不做运行时增删）
//!
//! 命令本身是幂等的覆盖写，但**调用点只有一处**：宿主开这个会话的 actor 时，且只在
//! 「这是一个全新会话」那一支。会话中途换 skill 索引 = 前缀缓存那一刻全断，
//! `docs/HOST-CAPABILITIES.md` §三 明确不做。

use crate::graph::{AtomKey, Slot};
use crate::value::host_skills::{self, HostSkill};

use super::session::Session;

impl Session {
    /// 记下宿主为这个会话声明的 skill：一条 `Entry`（label `"declare_host_skills"`）。
    ///
    /// 落在 **root** 头上（会话级命令，同 [`Session::declare_host_tools`]）——声明属于
    /// 这个会话，不属于树上某一个 agent。值经 `host_skills::to_value` **按 id 排序**
    /// 再落盘（红线 11，理由见那个模块）。
    ///
    /// 传空 `Vec` 是一次真正的空操作：值跟默认值相等，`record_set` 不产生 `Change`，
    /// `History::append` 不落条目——「没声明的会话」因此连一条幽灵 entry 都没有。
    pub fn declare_host_skills(&mut self, skills: Vec<HostSkill>) {
        let key = self.host_skills_key();
        let value = host_skills::to_value(skills);
        self.commit("declare_host_skills", |txn| txn.set_key(key, value));
    }

    /// 这个会话被声明过的 skill（**按 id 排序**——写入时排过，读回就是有序的）。
    ///
    /// 宿主（`agent-server` 的 actor）在建 `SkillRegistry` 时读它。空 = 没有任何注入，
    /// registry 为空 → 工具表不接 `.with_skills(..)`、索引段为空文本，这个会话跟
    /// 064 之前逐字节相同。
    pub fn host_skills(&self) -> Vec<HostSkill> {
        let key = self.host_skills_key();
        // 非创建读（`peek`，同 `host_tools`）：这个槽位在 `build_agent` 建图时就有
        // 默认值，兜底那一支只在「宿主拼错了 root id」时走得到。
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        host_skills::from_value(&value)
    }

    /// 声明只落在 root 头上（会话级）。两个口共用一处，免得一处写 root、另一处写
    /// 别的 agent，读写从此对不上——那正是静默错值的长相。
    fn host_skills_key(&self) -> AtomKey {
        AtomKey::Agent(self.agent.clone(), Slot::HostSkills)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ids::{AgentId, SkillId};
    use crate::value::tool::ToolSpec;

    use super::*;

    fn skill(id: &str) -> HostSkill {
        HostSkill {
            id: SkillId::new(id),
            description: Arc::from("一行描述"),
            body: Arc::from("正文若干"),
            tools: vec![ToolSpec {
                name: Arc::from("web:x/y"),
                description: Arc::from("说明"),
                schema: Arc::new(serde_json::json!({ "type": "object" })),
            }],
        }
    }

    /// 声明 → 读回（排序）；这是**一条 journaled entry**，undo 一下就回到没有注入
    /// 的状态——「undo 白拿」在 core 这一层的落点。
    #[test]
    fn a_declaration_is_journaled_and_undo_takes_it_back() {
        let mut s = Session::new(AgentId::root());
        assert!(s.host_skills().is_empty(), "全新会话没有任何注入的 skill");

        let before = s.history_len();
        s.declare_host_skills(vec![skill("zeta-flow"), skill("alpha-flow")]);
        assert_eq!(s.history_len(), before + 1, "声明是一条 journaled entry，不是一个不进日志的构造参数");

        let ids: Vec<String> = s.host_skills().iter().map(|sk| sk.id.as_str().to_string()).collect();
        assert_eq!(ids, vec!["alpha-flow", "zeta-flow"]);
        assert_eq!(&*s.host_skills()[0].body, "正文若干");

        let report = s.undo_step();
        assert!(matches!(report, crate::command::UndoReport::Applied { .. }), "{report:?}");
        assert!(s.host_skills().is_empty(), "undo 越过声明那一步之后，这个会话不该还认得那些 skill");

        let _ = s.redo_step();
        assert_eq!(s.host_skills().len(), 2);
    }

    /// 空声明不落 entry：不带 `capabilities` 的会话，日志跟 064 之前一个字节不差。
    #[test]
    fn declaring_nothing_leaves_no_trace() {
        let mut s = Session::new(AgentId::root());
        let before = s.history_len();
        s.declare_host_skills(Vec::new());
        assert_eq!(s.history_len(), before, "空声明不该留下一条幽灵 entry");
        assert!(s.host_skills().is_empty());
    }
}
