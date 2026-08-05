//! skill 跟工具表的关系（039 开的口，064 补上撞名过滤）。
//!
//! 从 `tool_table.rs` 分出来的一件事：registry 为什么归表拥有、每一轮怎么把激活集
//! 展开成注入料、以及**表里已经有的名字**怎么从 `late_tools` 里滤掉。工具表的五档
//! 装配、名字规则、`snapshot` 的三级判定留在 [`super`]。
//!
//! # registry 为什么归表拥有
//!
//! [`ToolTable::with_skills`] 一次把「声明两个 skill 工具」和「拥有 registry 供
//! dispatch 随时查」绑在一起——两件事本来就是一件（开了 skill 才有那两个工具，也才
//! 需要 registry）。分成 `RunnerCtx` 上的两个字段就会长出「表里有 activate、
//! registry 却是空的」这种半开状态。
//!
//! # 跨路径撞名：**表赢，多余那份在这里滤掉**（069 §拍板 第 2 问，064 §范围 第 5 条）
//!
//! 宿主注入的 `web:foo` 进了工具表，而某个 skill 激活时 `late_tools` 里也带一个
//! `web:foo`——两份同名 spec 一起进 prompt，正是 069 那条红线（「一个名字在进 prompt
//! 的那张表里只能出现一次，且它的描述/schema 必须就是 dispatch 真会执行的那一份」）
//! 要挡的东西。[`ToolTable::skill_injection`] 在返回前把「表里已经有的名字」从
//! `late_tools` 里滤掉。
//!
//! 四条理由，逐条对应 069 §拍板 第 2 问：
//!
//! 1. **赢家不是这里选的，是 dispatch 早就定死的。** [`ToolTable::declares`] 为真是
//!    因为**表**里有它，远端第五路于是把调用派给宿主注册的那一份；skill 带的那份
//!    spec **从来没有过自己的执行路径**。滤掉它**执行侧一个字节都不变**，变的只是
//!    「不再给模型看一份它影响不了的 schema」。这是四个子问题里唯一一个不需要权衡的
//!    ——现状严格劣于结论。
//! 2. **这里绝不能报错。** `skill_injection` **每轮都跑**（`provider_call::start` 组
//!    料时），而写下那份声明的人早就不在场了。轮中失败 = 会话在最坏的时刻死，且没有
//!    任何人能修它，正好是 069 那条判据（「撞名一律在最早能报给有权修它的人的那个点
//!    上失败」）的反面。
//! 3. **也不能指望上游全拒。** 061 的校验只看**一份声明**（顶层 tools + 每个 skill
//!    自带的 tools 判全局唯一），管不到「宿主声明的工具」撞上「别的来源装载进来的
//!    skill 带的工具」——`agent-cli` 从磁盘 `./skills/` 装载的 skill 就在这条路上，
//!    它跟内置五档的名字没有任何共同的裁判点。
//! 4. **滤完仍然满足红线 11**：`active` 已排序，滤的判据是「表里有没有」——跟轮次
//!    无关、幂等，同一份激活集渲染出的字节仍逐字节相同。
//!
//! **滤的是工具，不是 skill**：`late_system` 里那个 skill 的正文一个字节不少。撞名
//! 是工具名的事，跟这个 skill 该不该被激活、正文该不该注入没有关系。

use std::sync::Arc;

use agent_core::{SkillId, SystemChunk, ToolCallRequest, ToolSpec};
use serde_json::Value;

use crate::skill::{SkillRegistry, activate_spec, deactivate_spec};

use super::ToolTable;

impl ToolTable {
    /// 039 开闸：追加 `srv:skill/activate` + `srv:skill/deactivate`，并把宿主装载的
    /// `registry` 交给这张表拥有（供 dispatch 截获时查正文/工具、供 `provider_call`
    /// 组每一轮的 `late_system`/`late_tools`）。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死的
    /// 既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    ///
    /// **常驻索引不在这里**——它是 system 段的一部分（不是工具），由宿主调
    /// `registry.skill_index_chunk()` 放进 `Ingredients::system`（`agent-cli` 的
    /// `main.rs` 与 `agent-server` 的 `actor::body` 各有一处）。
    ///
    /// **空 registry 时不要调它**（064）：调了就等于给一个没有任何 skill 的会话平白
    /// 加两个永远没用的工具，而工具表在 prompt 最前面——那是所有会话共有的那一段
    /// 字节，只该在宿主真的开了 skill 时才变。
    pub fn with_skills(mut self, registry: SkillRegistry) -> Self {
        self.push_spec(activate_spec());
        self.push_spec(deactivate_spec());
        self.registry = registry;
        self
    }

    /// 这张表拥有的 skill registry（dispatch 截获激活时查它）。没开 skill 时是空的。
    pub(crate) fn skill_registry(&self) -> &SkillRegistry {
        &self.registry
    }

    /// 把一组激活的 skill 展开成本轮的注入料（正文 → `late_system`、工具 →
    /// `late_tools`）。`provider_call::start` 组料时调它。
    ///
    /// **表里已经有的名字从 `late_tools` 里滤掉**（069/064，理由见模块文档）：
    /// 不报错、不失败、不改 `late_system`。
    pub(crate) fn skill_injection(&self, active: &[SkillId]) -> (Vec<SystemChunk>, Vec<ToolSpec>) {
        let (late_system, mut late_tools) = self.registry.injection(active);
        late_tools.retain(|spec| !self.declares(&spec.name));
        (late_system, late_tools)
    }

    /// 只解析当前 agent 已激活的 host skill 工具。表里的同名声明永远优先走既有路径；
    /// registry 内部再负责来源、远端前缀、唯一性与 reversibility 的 fail-closed 判定。
    pub(crate) fn active_host_tool_request(
        &self,
        active: &[SkillId],
        name: &str,
        input: Arc<Value>,
    ) -> Option<ToolCallRequest> {
        if self.declares(name) {
            return None;
        }
        self.registry.active_host_tool_request(active, name, input)
    }
}

#[cfg(test)]
#[path = "tool_table_skill_tests.rs"]
mod tests;
