//! [`Session::active_skills`] / [`Session::active_skills_of`]：`Slot::SkillsActive`
//! 的**只读**口（141 之后）。
//!
//! ## 141：写入点已删，槽位留壳
//!
//! 决策 21（M5）曾经让模型经 `srv:skill/activate` / `srv:skill/deactivate` 两个
//! 内置工具写这个槽位（`Session::activate_skill` / `deactivate_skill` + 一个
//! `SkillError`）；决策 27（M15）把这条路整个换掉——skill 正文改成 `srv:skill/read`
//! 按需取（137/139），不再有「激活」这个动作。[141](../../../docs/issues/141-remove-activation-subsystem.md)
//! 删掉了那两个写入方法和 `SkillError`，**`Slot::SkillsActive` 本身留着**：
//! 红线 4（落盘用 `AtomKey`）——删变体会让老会话（journal 里真有 `activate_skill`
//! entry）反序列化直接断。
//!
//! ## 恢复老会话之后：状态在，没人读
//!
//! 这是一处**如实的行为变化**，不是「兼容」：老会话恢复回来，`active_skills_of`
//! 仍然能读出当年激活过的 id 集合（状态没丢），但**没有任何生产代码再拿它去组
//! 下一轮的请求体**（`agent-runtime` 那个曾经把激活集展开成注入料的方法已经
//! 随 141 删掉）——恢复之后继续对话，新一轮的请求体里不会再出现那个 skill 的正文。
//! 唯一还在读这个槽位的是 `agent-cli` 的 `/skills` 展示（纯状态回显，不进 prompt）。
//!
//! ## store 只存激活集，正文在 registry（TOOLS.md §Skills）
//!
//! 这里只管「哪些 skill id 曾经被激活过」，skill 的正文活在 store 外的
//! `SkillRegistry`（`agent-runtime`，红线 7：core 不做 IO）。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::{AgentId, SkillId};
use crate::value::str_set;

use super::session::Session;

impl Session {
    /// **root** 当前激活的 skill id（**排序**——写入时排序去重，读回就是有序的）。
    ///
    /// 跟 `messages()` / `status()` 一样是「per-agent 读口的 root 特化」：不带参数的
    /// 这个读的是 root，带参数的 [`active_skills_of`](Session::active_skills_of) 读
    /// 任意 agent，**同一条实现**——分成两条的那一刻，root 和子 agent 的读取就会
    /// 开始悄悄分叉（read.rs 顶部「per-agent 取料口」的同一条约定）。
    pub fn active_skills(&self) -> Vec<SkillId> {
        self.active_skills_of(&self.agent)
    }

    /// 这个 agent 曾经激活过的 skill id（排序）。141 之前是「宿主用它去 registry
    /// 现取正文/工具，组这一轮的注入料」；141 之后**没有生产代码再调用它做这件事**
    /// ——留着只为 `agent-cli` 的 `/skills` 状态展示，和老会话的这个槽位本来就有
    /// 数据、总要有个读口对得上。空 = 没激活过任何 skill。
    pub fn active_skills_of(&self, agent: &AgentId) -> Vec<SkillId> {
        self.active_skill_names(agent)
            .into_iter()
            .map(SkillId)
            .collect()
    }

    /// 激活集里的裸 id（`Arc<str>`）。公开读口 [`active_skills`](Session::active_skills)
    /// 走它——非创建读（`peek`），探一个不在树上的 id 不该在 family 里留下一个
    /// 没人写的 atom（跟 `tools_allowed_of` 同一条判断）。
    fn active_skill_names(&self, agent: &AgentId) -> Vec<Arc<str>> {
        let key = AtomKey::Agent(agent.clone(), Slot::SkillsActive);
        let value = self.peek(&key).unwrap_or_else(|| key.default_value());
        str_set::from_value(&value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 141 之后没有写入点：新建的会话这个槽位永远是空集，跟它从来没被写过的
    /// 默认值一样——这条钉住「留壳不留写口」的那一半。
    #[test]
    fn a_fresh_session_has_no_active_skills() {
        let s = Session::new(AgentId::root());
        assert!(s.active_skills().is_empty());
        assert!(s.active_skills_of(&AgentId::new("other/a1")).is_empty());
    }
}
