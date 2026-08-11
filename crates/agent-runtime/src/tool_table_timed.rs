//! 工具表的第三个正交维度：**调用时机**（133，M15 全部机制的地基，决策 27）。
//!
//! `location`/`reversibility` 答的是「这个工具在哪跑、undo 时怎么办」；这一维答的是
//! **谁发起调用**——空（不在这个类型里，是「不在 timed 区」本身）= 模型自主调（今天
//! 的全部工具），`SessionStart` = 会话创建时 runtime 自动调一次，`TurnEnd` = 每个
//! 完成轮后 runtime 自动调一次。三维正交：一个 `SessionStart` 工具照样有自己的
//! `Server`/`Pure` 之类的判定，位置和可逆性跟今天任何工具算法一样，不因为进了 timed
//! 区就换一套规则。
//!
//! `CallTiming` 定义在 `agent-runtime`，不进 `agent-core`——core 连「工具由谁发起」
//! 都不该知道（红线 12 的精神：core 只有一条路径，「什么时候自动调一个工具」是宿主
//! 编排的事，不是状态机的事）。
//!
//! # timed 工具住独立区，不混进 `specs`
//!
//! 076 disable 那条判据的延续：**「表里有什么」和「模型看得见什么」必须是同一个
//! 答案**，`declares()` 只能回答一个。timed 工具因此另开一个 `Vec<TimedTool>`
//! （[`ToolTable`] 的私有字段 `timed_tools`），`specs()`/`declares()`/`snapshot()`
//! 一个字节看不见它——喂模型的那份表照旧只由 `specs` 那个 `Vec` 决定。模型硬猜出
//! 一个 timed 工具的名字发 ToolCall，走既有 `unknown_tool` 路，不需要任何新判断。
//!
//! # 执行体是注册时给的本地函数，不走 dispatch
//!
//! timed 条目自带执行体（[`TimedRun`]），135/136 的驱动直接调 [`TimedTool::run`]，
//! **不经过** `dispatch`/`ToolExecutor`/远端等待槽那一整套。这不是抄近路，是 v1 的
//! 结构性事实：会话创建那一刻 SSE 还没接上，一个 `Web` 位置的开局工具永远等不到
//! 回写；与其加一层「这个时机只允许哪些位置」的运行时校验，不如让远端执行体在
//! **签名上**就不存在——`TimedRun` 只能是 `Fn(&ToolTable, &Value) -> Result<...>`，
//! 没有 async、没有 effect、没有 epoch。要支持远端/MCP 时机工具是将来的显式扩展，
//! 不是这条签名的隐藏能力。
//!
//! 执行体拿 `&ToolTable` 自身（不是绑死某个驱动实例），这样它能读到表内数据
//! （比如 138 要用的索引函数）而不必让 `ToolTable` 反过来认识调用它的是谁。
//!
//! # 撞名：双向查
//!
//! `with_timed` 装的名字可能撞 specs 区（已经喂模型的名字被一个模型看不见的工具
//! 偷走一次执行路径，直接违反「一个名字一条执行路径」），[`ToolTable::push_spec`]
//! 也可能撞已经注册过的 timed 名——两者的调用顺序不是这个文件能控制的（装配链里
//! `with_timed` 插在哪一步全看调用方怎么写），所以查重必须双向，不能只查一侧。
//! 跟 075 的 `push_spec` 同一个判据：作者是程序员（装配代码），落 `debug_assert!` +
//! release 静默丢弃，不属于「运行时数据不能硬失败」那一类。

use std::sync::Arc;

use agent_core::ToolSpec;
use serde_json::Value;

use super::ToolTable;

/// 工具的调用时机。**不含「模型自主调」这一档**——那是默认状态，靠「不在 timed 区」
/// 表达，不需要一个变体去命名「没有特殊时机」。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CallTiming {
    /// 会话创建时 runtime 自动调一次。
    SessionStart,
    /// 每个完成轮后 runtime 自动调一次。
    TurnEnd,
}

/// timed 工具的执行体：拿 `&ToolTable`（读表内数据）和这次调用的 input，本地同步
/// 跑完直接给结果——不产出 effect，不进 dispatch，理由见模块文档。
pub type TimedRun = Box<dyn Fn(&ToolTable, &Value) -> Result<Arc<str>, Arc<str>> + Send + Sync>;

/// 一条 timed 工具：模型看不见的 spec（留给驱动/索引读 name/description 用）、
/// 它的调用时机、它的执行体。三个字段都私有——外部只能经 [`TimedTool::spec`] 和
/// [`TimedTool::run`] 两个方法碰它，`timing` 只在本模块内部用于按时机过滤。
pub struct TimedTool {
    spec: ToolSpec,
    timing: CallTiming,
    run: TimedRun,
}

impl TimedTool {
    /// 这条 timed 工具的静态声明。**不进 `Ingredients::tools`**——喂模型的那份表
    /// 由 [`ToolTable::specs`] 单独决定，这个返回值只给驱动或 138 的索引函数读
    /// name/description 用。
    pub fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    /// 跑一次这条 timed 工具。`table` 是它注册所在的那张表本身——执行体因此能
    /// 读到表内数据（`&ToolTable` 是共享借用，改不了），不需要 `ToolTable` 反过来
    /// 认识调用方是谁。
    pub fn run(&self, table: &ToolTable, input: &Value) -> Result<Arc<str>, Arc<str>> {
        (self.run)(table, input)
    }
}

impl ToolTable {
    /// 133：注册一条 timed 工具。`spec` 不进 `specs`（模型看不见），`timing` 决定
    /// 它归哪个驱动管，`run` 是它的执行体。
    ///
    /// **撞名双向查**（specs 区 + timed 区内部）→ `debug_assert!` + release 下整条
    /// 丢弃，不 push。理由与判据同 [`ToolTable::push_spec`]。
    pub fn with_timed(mut self, spec: ToolSpec, timing: CallTiming, run: TimedRun) -> Self {
        if self.declares(&spec.name) || self.declares_timed(&spec.name) {
            debug_assert!(
                false,
                "ToolTable 已经有工具 `{}` 了（specs 区或 timed 区），撞名的这条 \
                 timed 工具整条丢弃",
                spec.name
            );
            return self;
        }
        self.timed_tools.push(TimedTool { spec, timing, run });
        self
    }

    /// timed 区是否已经有这个名字。`with_timed` 撞名双向查的一侧；另一侧是
    /// `pub(super)`，给 `push_spec` 反向查用——specs 区的新名字也不能撞已经注册
    /// 过的 timed 名（见那边的调用点与本模块文档「撞名：双向查」）。
    pub(super) fn declares_timed(&self, tool: &str) -> bool {
        self.timed_tools.iter().any(|t| &*t.spec.name == tool)
    }

    /// 按注册顺序迭代某个时机的 timed 工具（135/136 各自的驱动用）。timed 区本身
    /// 是 `Vec`，push 顺序即注册顺序，这里只按 `timing` 过滤，不重排。
    pub fn timed(&self, timing: CallTiming) -> impl Iterator<Item = &TimedTool> {
        self.timed_tools.iter().filter(move |t| t.timing == timing)
    }
}

#[cfg(test)]
#[path = "tool_table_timed_tests.rs"]
mod tests;
