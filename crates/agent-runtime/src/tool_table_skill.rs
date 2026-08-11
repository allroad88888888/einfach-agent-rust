//! skill 跟工具表的关系（039 开的口，064 补上撞名过滤，139 切装配到 read/index）。
//!
//! 从 `tool_table.rs` 分出来的一件事：registry 为什么归表拥有、每一轮怎么把激活集
//! 展开成注入料、以及**表里已经有的名字**怎么从 `late_tools` 里滤掉。工具表的五档
//! 装配、名字规则、`snapshot` 的三级判定留在 [`super`]。
//!
//! # registry 为什么归表拥有
//!
//! [`ToolTable::with_skills`] 一次把「声明 skill 相关工具」和「拥有 registry 供
//! dispatch/驱动随时查」绑在一起——两件事本来就是一件（开了 skill 才有这些工具，也才
//! 需要 registry）。分成 `RunnerCtx` 上的两个字段就会长出「表里有 read、registry
//! 却是空的」这种半开状态。
//!
//! # 139：装配切到 read（specs）+ index（timed），不再是 activate/deactivate
//!
//! `with_skills` 曾经追加 `srv:skill/activate` + `srv:skill/deactivate`（039）、
//! 由宿主另外把 `registry.skill_index_chunk()` 塞进 `Ingredients::system`。139
//! 把这条装配换成：`srv:skill/read` 进 specs（模型面按 id 现取正文）、
//! `srv:skill/index`（138）挂进 `SessionStart` 时机区（133），135 的开局驱动在
//! 新建会话那一刻跑它一次，结果落进 `Session` 的前缀块——常驻这件事没变，只是
//! 从「宿主手动拼一段 system」换成「跟工具表一样是装配的一部分」。
//!
//! **只切装配，不删机制**：`Slot::SkillsActive`、`skill::intercept`（激活/停用的
//! dispatch 截获）、[`ToolTable::skill_injection`] 一个字节不动——`with_skills`
//! 不再注册 `srv:skill/activate`/`srv:skill/deactivate` 这两个名字，`dispatch.rs`
//! 里那两条截获路由因此对**新会话**恒为死代码（`declares()` 假），但代码本身留着：
//! 老会话（journal 里已经有激活记录）恢复后 `skill_injection` 照样按
//! `Slot::SkillsActive` 展开注入，不因为新会话不再产生新的激活而失效。删掉这套
//! 机制是 141（`docs/issues/141-remove-activation-subsystem.md`）的事。
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

use crate::skill::{SkillRegistry, index_spec, read_spec};

use super::{CallTiming, ToolTable};

impl ToolTable {
    /// 139 开闸：追加 `srv:skill/read`（模型面 specs，按 id 现取正文）+
    /// `srv:skill/index`（138，挂进 `SessionStart` 时机区，133），并把宿主装载的
    /// `registry` 交给这张表拥有——供 dispatch 截获 read 时查正文（`skill/read.rs`）、
    /// 供 index 的执行体读 `index_text()`、供 `provider_call` 组每一轮的
    /// `late_system`/`late_tools`。
    ///
    /// index 的执行体只拿 `&ToolTable` 本身（`with_timed` 的签名，见
    /// `tool_table_timed.rs` 模块文档「执行体拿 `&ToolTable` 自身」）——它不捕获
    /// `registry`，而是在真正跑的那一刻经 [`ToolTable::skill_registry`] 现查，
    /// 这样 135 的开局驱动跑它时读到的永远是**这张表**最终装配完的那份 registry，
    /// 不会因为闭包提前捕获了一份浅拷贝而跟 `read`/`skill_injection` 看到两份。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死的
    /// 既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    ///
    /// **常驻索引不再是这里手写的 system chunk**——135 的开局驱动在新建会话那一刻
    /// 跑一次 `SessionStart` 时机区，把 index 的产出落进 `Session` 的前缀块
    /// （`session.prefix_chunks()`），跟工具表一样是稳定前缀的一部分，只是不需要
    /// 宿主（`agent-cli`/`agent-server`）再手动拼一段 `Ingredients::system` 了。
    ///
    /// **空 registry → 什么都不接**（064 的判据，139 起由这个函数自己守住，不再
    /// 只靠调用方自觉）：接了就等于给一个没有任何 skill 的会话平白加一个永远没用
    /// 的工具（`srv:skill/read`）和一条永远回空文本的开局工具——工具表在 prompt
    /// 最前面，那是所有会话共有的那一段字节，只该在宿主真的开了 skill 时才变。
    /// `self.registry` 仍然照收（哪怕是空的）：跟不收相比字节上无差别
    /// （`skill_registry()`/`skill_injection()` 都不进 `specs()`），但省得调用方
    /// 还要记着「registry 是空的时候这一步该跳过」——`agent-cli` 的装配链就是
    /// 无条件调它，靠的正是这里兜底。
    pub fn with_skills(mut self, registry: SkillRegistry) -> Self {
        if registry.is_empty() {
            self.registry = registry;
            return self;
        }
        self.push_spec(read_spec());
        self.registry = registry;
        self.with_timed(
            index_spec(),
            CallTiming::SessionStart,
            Box::new(|table, _input| Ok(table.skill_registry().index_text())),
        )
    }

    /// 这张表拥有的 skill registry（dispatch 截获 read 时查它，index 的 timed 执行体
    /// 也查它）。没开 skill 时是空的。
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

/// 139：`with_skills` 新装配形状本身的单测（specs/timed 断言、二次调用判重、
/// 老会话兼容）——跟上面 `tests`（跨路径撞名，069/064）是两个不同的主题，
/// 分两个文件（`tool_table.rs` 的 `tool_table_tests.rs` + `standard_local_tests.rs`
/// 已经是这个先例）。
#[cfg(test)]
#[path = "tool_table_skill_assembly_tests.rs"]
mod assembly_tests;
