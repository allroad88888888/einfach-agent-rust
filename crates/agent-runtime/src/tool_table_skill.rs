//! skill 跟工具表的关系（039 开的口，139 切装配到 read/index，141 删掉激活式
//! 注入与它专属的执行授权）。
//!
//! 从 `tool_table.rs` 分出来的一件事：registry 为什么归表拥有。工具表的五档装配、
//! 名字规则、`snapshot` 的三级判定留在 [`super`]。
//!
//! # registry 为什么归表拥有
//!
//! [`ToolTable::with_skills`] 一次把「声明 skill 相关工具」和「拥有 registry 供
//! dispatch/驱动随时查」绑在一起——两件事本来就是一件（开了 skill 才有这些工具，也才
//! 需要 registry）。分成 `RunnerCtx` 上的两个字段就会长出「表里有 read、registry
//! 却是空的」这种半开状态。
//!
//! # 139：装配是 read（specs）+ index（timed），不是 activate/deactivate
//!
//! `with_skills` 追加 `srv:skill/read` 进 specs（模型面按 id 现取正文）、
//! `srv:skill/index`（138）挂进 `SessionStart` 时机区（133），135 的开局驱动在
//! 新建会话那一刻跑它一次，结果落进 `Session` 的前缀块。
//!
//! # 141：激活式注入与跨路径撞名过滤已删
//!
//! 039 期这里还有一个方法——把激活集展开成正文段/中途工具两份注入料，并把
//! 「表里已经有的名字」从中途工具那份里滤掉（069/064 那条撞名红线）。决策 27
//! 把 skill 携带工具的能力整个砍了（`capabilities.skills[].tools` 非空已经在
//! server 侧 400，见 140），激活机制也随 141 一起删——`late_tools` 从此没有任何
//! 生产者，撞名过滤连同它一起没有存在的必要。`Ingredients.late_tools` 字段本身
//! 没删（留给别的、非 skill 的中途加工具场景），`provider_call::start` 起恒传空。

use crate::skill::{SkillRegistry, index_spec, read_spec};

use super::{CallTiming, ToolTable};

impl ToolTable {
    /// 139 开闸：追加 `srv:skill/read`（模型面 specs，按 id 现取正文）+
    /// `srv:skill/index`（138，挂进 `SessionStart` 时机区，133），并把宿主装载的
    /// `registry` 交给这张表拥有——供 dispatch 截获 read 时查正文（`skill/read.rs`）、
    /// 供 index 的执行体读 `index_text()`、供 `srv:skill/read` 按 id 现取正文。
    ///
    /// index 的执行体只拿 `&ToolTable` 本身（`with_timed` 的签名，见
    /// `tool_table_timed.rs` 模块文档「执行体拿 `&ToolTable` 自身」）——它不捕获
    /// `registry`，而是在真正跑的那一刻经 [`ToolTable::skill_registry`] 现查，
    /// 这样 135 的开局驱动跑它时读到的永远是**这张表**最终装配完的那份 registry，
    /// 不会因为闭包提前捕获了一份浅拷贝而跟 `read` 看到两份。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死的
    /// 既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    ///
    /// **常驻索引不是这里手写的 system chunk**——135 的开局驱动在新建会话那一刻
    /// 跑一次 `SessionStart` 时机区，把 index 的产出落进 `Session` 的前缀块
    /// （`session.prefix_chunks()`），跟工具表一样是稳定前缀的一部分，不需要
    /// 宿主（`agent-cli`/`agent-server`）手动拼一段 `Ingredients::system`。
    ///
    /// **空 registry → 什么都不接**（064 的判据，139 起由这个函数自己守住，不再
    /// 只靠调用方自觉）：接了就等于给一个没有任何 skill 的会话平白加一个永远没用
    /// 的工具（`srv:skill/read`）和一条永远回空文本的开局工具——工具表在 prompt
    /// 最前面，那是所有会话共有的那一段字节，只该在宿主真的开了 skill 时才变。
    /// `self.registry` 仍然照收（哪怕是空的）：跟不收相比字节上无差别
    /// （`skill_registry()` 不进 `specs()`），但省得调用方还要记着「registry 是空
    /// 的时候这一步该跳过」——`agent-cli` 的装配链就是无条件调它，靠的正是这里兜底。
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
}

/// 139：`with_skills` 装配形状本身的单测（specs/timed 断言、二次调用判重、
/// 老会话兼容——141 之后是「恢复不 panic、不再注入」）。
#[cfg(test)]
#[path = "tool_table_skill_assembly_tests.rs"]
mod assembly_tests;
