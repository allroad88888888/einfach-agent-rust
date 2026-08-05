//! [`Session::activate_skill`] / [`Session::deactivate_skill`] / [`Session::active_skills`]：
//! skill 激活集的读写（039）。
//!
//! 决策 21（M5）：**skill 由模型经内置工具激活**，激活即一次 tool call，记账走
//! 既有的 command 层。这个文件是那条决策在状态侧的落点——`agent-runtime` 的
//! `srv:skill/activate` / `srv:skill/deactivate` 工具截获之后调这里。
//!
//! ## 为什么是 command 而不是裸 `store.set`（红线 2/4）
//!
//! 激活是一次 **journaled** 的状态变更：写 `Slot::SkillsActive` 走 `commit_as` 留下
//! 一条 `Entry`，于是 `/undo` 连激活一起退掉是**白拿的**——跟别的 primitive 一视
//! 同仁，undo 撤一次激活就退化成一次普通的值回滚。这正是 M5 验收整句里
//! 「`/undo` 连激活一起退掉」的落点，不需要任何额外机制。
//!
//! ## store 只存激活集，正文/工具在 registry（TOOLS.md §Skills）
//!
//! 这里只管「哪些 skill id 被激活」，skill 的正文（激活时注入 `late_system`）和它
//! 携带的工具（进 `late_tools`）活在 store 外的 `SkillRegistry`（`agent-runtime`，
//! 红线 7：core 不做 IO）。恢复时激活集这个 primitive 自动回来、正文从 registry
//! **现取**——registry 内容在两次运行之间漂移了（skill 改了正文、删了一个 skill），
//! 激活集里那个 id 要么取到新正文、要么取不到（registry 侧当它没激活）。这个漂移
//! 语义是刻意的：store 里存正文才能保证「恢复出来逐字节一致」，但那会把一份可能很大
//! 的资产复制进每一条快照，且 skill 更新后老会话永远拿旧正文——两害相权，存 id。
//!
//! ## 落值前排序去重（红线 11）
//!
//! 激活集会被 registry 展开成注入进 system prompt 的正文，顺序一漂前缀缓存就全价。
//! 写入走 `value::str_set`（跟 spawn 的工具子集同一处编解码），排序去重逐字节确定。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::{AgentId, SkillId};
use crate::value::str_set;

use super::session::Session;

/// 激活/停用一个 skill 被拒的理由。**全部是可预期的拒绝**，不是 bug——
/// `agent-runtime` 的 skill 工具把它翻成 `is_error` 的 tool_result 喂回模型
/// （跟 spawn 的 [`SpawnRefused`](super::SpawnRefused) 同一套哲学）。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SkillError {
    /// 这个 id 不在本会话这棵树上（跨 root 不共享 store）。
    NotInSession { agent: AgentId },
    /// 这个 agent 不在活名单上：从没 spawn 过、spawn 被 undo 撤了、或者已经 despawn。
    /// 给一个死掉的 agent 激活 skill 没有意义——它的槽位马上就是墓碑。
    NotLive { agent: AgentId },
    /// 这个 skill 已经在激活集里了（激活是幂等的语义边界，但如实回报比静默
    /// 吞掉更有用：模型据此知道不用再激活一次）。
    AlreadyActive { agent: AgentId, skill: SkillId },
    /// 这个 skill 本来就不在激活集里，停用无从谈起。
    NotActive { agent: AgentId, skill: SkillId },
}

impl Session {
    /// 在 `agent` 上激活一个 skill：把它的 id 加进 `Slot::SkillsActive`。
    ///
    /// 一条 `Entry`（label `"activate_skill"`），继承所在 root turn 的 `turn_id`
    /// ——于是 `undo_turn` 一次退回一整轮时，这次激活跟那一轮别的工作同进同退。
    ///
    /// 已经激活的再激活 → [`SkillError::AlreadyActive`]，**不落 `Entry`**
    /// （值没变，`record_set` 本来也不会落，但这里提前返回，连 `commit_as` 都不进——
    /// 那样连一条空 batch 都不会产生，`known_label` 也不必为「没改任何东西的激活」
    /// 留一条痕迹）。
    pub fn activate_skill(&mut self, agent: &AgentId, skill: SkillId) -> Result<(), SkillError> {
        self.check_skill_agent(agent)?;
        let mut active = self.active_skill_names(agent);
        if active.iter().any(|s| **s == *skill.as_str()) {
            return Err(SkillError::AlreadyActive {
                agent: agent.clone(),
                skill,
            });
        }
        active.push(Arc::clone(&skill.0));
        self.write_skills(agent, "activate_skill", active);
        Ok(())
    }

    /// 在 `agent` 上停用一个 skill：把它的 id 从 `Slot::SkillsActive` 里移掉。
    ///
    /// 一条 `Entry`（label `"deactivate_skill"`）。不在激活集里 →
    /// [`SkillError::NotActive`]，不落 `Entry`。
    pub fn deactivate_skill(&mut self, agent: &AgentId, skill: SkillId) -> Result<(), SkillError> {
        self.check_skill_agent(agent)?;
        let mut active = self.active_skill_names(agent);
        if !active.iter().any(|s| **s == *skill.as_str()) {
            return Err(SkillError::NotActive {
                agent: agent.clone(),
                skill,
            });
        }
        active.retain(|s| **s != *skill.as_str());
        self.write_skills(agent, "deactivate_skill", active);
        Ok(())
    }

    /// **root** 当前激活的 skill id（**排序**——写入时排序去重，读回就是有序的）。
    ///
    /// 跟 `messages()` / `status()` 一样是「per-agent 读口的 root 特化」：不带参数的
    /// 这个读的是 root，带参数的 [`active_skills_of`](Session::active_skills_of) 读
    /// 任意 agent，**同一条实现**——分成两条的那一刻，root 和子 agent 的读取就会
    /// 开始悄悄分叉（read.rs 顶部「per-agent 取料口」的同一条约定）。
    pub fn active_skills(&self) -> Vec<SkillId> {
        self.active_skills_of(&self.agent)
    }

    /// 这个 agent 当前激活的 skill id（排序）。宿主（`agent-runtime`）用它去 registry
    /// 现取正文/工具，组这个 agent 这一轮的 `late_system` / `late_tools`。
    /// 空 = 没激活任何 skill。
    pub fn active_skills_of(&self, agent: &AgentId) -> Vec<SkillId> {
        self.active_skill_names(agent)
            .into_iter()
            .map(SkillId)
            .collect()
    }

    /// 激活集里的裸 id（`Arc<str>`）。公开读口 [`active_skills`](Session::active_skills)
    /// 和两个 mutator 都走它——非创建读（`peek`），探一个不在树上的 id 不该在
    /// family 里留下一个没人写的 atom（跟 `tools_allowed_of` 同一条判断）。
    fn active_skill_names(&self, agent: &AgentId) -> Vec<Arc<str>> {
        let key = AtomKey::Agent(agent.clone(), Slot::SkillsActive);
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        str_set::from_value(&value)
    }

    /// 两个 mutator 共用的落值：一条命令把新激活集写进槽位（排序去重在
    /// `str_set::to_value`）。记在 `agent` 名下——是它下的这次工具调用。
    fn write_skills(&mut self, agent: &AgentId, label: &'static str, active: Vec<Arc<str>>) {
        let key = AtomKey::Agent(agent.clone(), Slot::SkillsActive);
        let value = str_set::to_value(active);
        self.commit_as(agent, label, |txn| txn.set_key(key, value));
    }

    /// 激活/停用共用的前两道闸：在本会话、且活着。
    fn check_skill_agent(&self, agent: &AgentId) -> Result<(), SkillError> {
        if !self.in_session(agent) {
            return Err(SkillError::NotInSession {
                agent: agent.clone(),
            });
        }
        if !self.is_live(agent) {
            return Err(SkillError::NotLive {
                agent: agent.clone(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        Session::new(AgentId::root())
    }

    /// 激活 → 读回；重复激活被拒且不落 entry；停用 → 读回空。
    #[test]
    fn activate_then_deactivate_round_trips_and_is_journaled() {
        let mut s = session();
        let root = AgentId::root();
        assert!(s.active_skills().is_empty());

        let before = s.history_len();
        s.activate_skill(&root, SkillId::new("foo")).unwrap();
        assert_eq!(s.active_skills(), vec![SkillId::new("foo")]);
        assert_eq!(s.history_len(), before + 1, "激活是一条 journaled entry");

        // 重复激活：被拒，历史不动。
        let err = s.activate_skill(&root, SkillId::new("foo")).unwrap_err();
        assert!(matches!(err, SkillError::AlreadyActive { .. }));
        assert_eq!(s.history_len(), before + 1, "重复激活不落幽灵 entry");

        s.deactivate_skill(&root, SkillId::new("foo")).unwrap();
        assert!(s.active_skills().is_empty());
        assert_eq!(s.history_len(), before + 2);

        // 停用一个不在集里的：被拒，历史不动。
        let err = s.deactivate_skill(&root, SkillId::new("foo")).unwrap_err();
        assert!(matches!(err, SkillError::NotActive { .. }));
        assert_eq!(s.history_len(), before + 2);
    }

    /// 别的树上的 id 一律 `NotInSession`。
    #[test]
    fn an_alien_agent_is_refused() {
        let mut s = session();
        let alien = AgentId::new("other/a1");
        assert!(matches!(
            s.activate_skill(&alien, SkillId::new("foo")),
            Err(SkillError::NotInSession { .. })
        ));
    }
}
