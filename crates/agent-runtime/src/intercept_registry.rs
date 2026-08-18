//! 146/147：截获式扩展工具的**装配期注册表**（决策 29 的正门）——这个文件只管
//! 「表怎么工作」：自借用陷阱怎么绕开、撞名判据是什么、查表之后怎么调用。
//! 「表被拿来做什么」分住另外两个文件（一个文件一件事，红线 9）：
//!
//! - [`crate::session_tool_ext`]：`SessionToolFn` 公开层，扩展作者写的闭包怎么
//!   适配成这张表要的形状。
//! - [`crate::builtin_intercepts`]：既有四条工具截获（spawn/collect/status/
//!   skill-read，147）迁进来之后，各自转发给谁。
//!
//! # 从手工 if 链到注册表，机制层不认识任何具体工具
//!
//! `dispatch.rs` 曾经排着四条手工截获，一个新工具一条新 `if`。这个文件把
//! 「按名字截获、拿 `Session` 干活」的**壳**升级成一张装配期建好的表：新的截获式
//! 工具 `register_session_tool` 一次即可，既有四条改用 [`RunnerCtx::
//! register_intercept`] 迁进同一张表。表本身不认识任何具体工具，只按名字转发
//! ——`dispatch.rs` 现在只剩一条查表分支。
//!
//! # 两层签名：内部层收 dispatch 的全部入参面，公开层收窄给扩展
//!
//! [`InterceptArgs`]/[`InterceptFn`] 是**内部层**（`pub(crate)`）：字段跟
//! `dispatch::run_effect` 的入参面逐个对齐（`session`/`ctx`/`subtree`/
//! `compactions`/`bus`/`agent`/`call_id`/`input`/`epoch`）。147 把既有四条截获
//! 迁进来时，`subtree` 真的被 spawn/collect 读到了；`compactions`/`bus` 仍然只为
//! 类型对齐留着——`Effect::Compact` 的路由这条不迁。
//!
//! `SessionToolFn`（[`crate::session_tool_ext`]）是**公开层**：收窄到
//! `&mut Session` + `&AgentId` + `&Value`，机制上够不着 `Subtree`/
//! `CompactSlots`/`IoBus`。既有四条**不经过**那一层的适配器：它们要碰的东西
//! （spawn/collect 读写 `Subtree`，spawn 后台模式一次产两个事件）装不进收窄
//! 签名，[`RunnerCtx::register_intercept`] 让它们把原始 `InterceptFn` 直接插
//! 进同一张表。
//!
//! # 自借用陷阱：闭包住在 `ctx` 里，调用时又要 `&mut ctx`
//!
//! 朴素写法会长这样：`ctx.session_tools.get(name)` 借出 `&InterceptFn`，紧接着
//! 拿这个引用去调用它，而闭包体需要 `InterceptArgs { ctx, .. }` 再借一次
//! `&mut ctx`——同一个 `ctx` 被两次借用，编译器直接拒绝（借用检查器看不出「查表」
//! 和「调用」之间那个引用其实已经用不着了）。
//!
//! 解法是**整张表包一层 `Arc`**（[`SessionToolRegistry`]）：装配期用
//! `Arc::get_mut` 原地改（这时是唯一持有者，`Arc::get_mut` 必中）；调用期先
//! `Arc::clone` 出一份独立句柄（[`SessionToolRegistry::snapshot`]），借用锚定在
//! 这份克隆上而不是 `ctx` 本身——`ctx` 因此完全空出来，可以随意再借 `&mut`。
//! `Arc::clone` 只是原子自增，不是深拷贝整张表。
//!
//! `Arc::make_mut`（`T: Clone` 那条路）走不通：`InterceptFn = Box<dyn Fn(..)>`
//! 不是 `Clone`，`BTreeMap<_, InterceptFn>` 因此也不是——这也是为什么装配期改表
//! 走的是 `get_mut` 不是 clone-on-write。
//!
//! # 撞名判据：一名一路
//!
//! [`RunnerCtx::register_intercept`]（本文件）与 `RunnerCtx::register_session_tool`
//! （[`crate::session_tool_ext`]）共用同一套三道闸（[`RunnerCtx::registrable`]）
//! ——同 `ToolTable::push_spec` / `with_timed` 的哲学（装配代码的错误，
//! `debug_assert!` 炸出来 + release 静默不注册，不为此让宿主进程崩，见那两个
//! 函数的文档）：
//!
//! 1. 名字不能撞 timed 区（133）——timed 工具有自己的驱动（`SessionStart`/
//!    `TurnEnd` 直接调 `TimedTool::run`，从不经过 `ExecuteTool`），一旦被这张表
//!    截获，一个 `declares()` 为假的名字（timed 名从不进 `specs`）反而会被执行，
//!    正是「宿主没声明就不截获」那道闸想挡的事。
//! 2. 名字必须已经在 `declares()`（工具表 specs 区）——spec 是声明、截获是执行
//!    路径，一名一路缺一不可：没声明就注册是死代码（模型永远看不到这个名字），
//!    反过来只声明不截获，调用会落进常规 `ExecuteTool` 路最终 `unknown_tool`。
//! 3. 名字不能撞已经注册过的截获——同名重复注册整条丢弃。
//!
//! # [`dispatch`] 不做任何包装
//!
//! 每一次截获都欠两笔账：`ToolExecuting` 可见性、红线 6 的屏障登记
//! （`mark_no_undo`）。这两笔账**在插进表之前就该还清**——`SessionToolFn`
//! 那层由 `adapt`/`announce`（[`crate::session_tool_ext`]）代还；既有四条自己在
//! `intercept` 函数体内部还（`crate::builtin_intercepts` 原样转发，不加包装）。
//! 所以 [`dispatch`] 只剩「查表 + 调用」——重新在这里包一层会让既有四条的
//! `ToolExecuting` 发两次，这正是 147 逐字节验收要揪住的事。

use std::collections::BTreeMap;
use std::sync::Arc;

use agent_core::{AgentId, Epoch, Session, ToolCallId};
use serde_json::Value;

use crate::compact_slot::CompactSlots;
use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::io_bus::IoBus;
use crate::subtree::Subtree;

/// 兼容性重导出：`SessionToolFn` 的正门在 [`crate::session_tool_ext`]（147 从这个
/// 文件拆出去的公开适配层），但它建立之前已经有代码按 `crate::intercept_registry::
/// SessionToolFn` 这条路径引用（148 的 `extension_pack.rs`/`tool_table_extension.rs`
/// 并行在写，不归这条 issue 改）——留一行重导出比去改别的 in-flight 文件更安全。
pub use crate::session_tool_ext::SessionToolFn;

/// 内部层：一次截获拿到的全部东西，字段跟 `dispatch::run_effect` 的入参面逐个
/// 对齐（147 迁移既有四条截获时不用改这个类型的骨架，只把 `input` 从 `&Value`
/// 改宽成 `&Arc<Value>`——见该字段自己的注释）。
pub(crate) struct InterceptArgs<'a> {
    pub session: &'a mut Session,
    pub ctx: &'a mut RunnerCtx,
    /// 147 迁移进来的 spawn/collect 读它（`crate::builtin_intercepts`）。
    pub subtree: &'a mut Subtree,
    // `Effect::Compact` 的路由这条**不迁**（模块文档「两层签名」一节）：这两个
    // 字段到 147 收尾时仍然只为跟 `run_effect` 的入参面严格对齐留着，没有任何
    // 闭包读它们。真要迁 `Effect::Compact`/MCP 相关截获进来那天再摘掉这行
    // allow。
    #[allow(
        dead_code,
        reason = "147 范围不含 Effect::Compact/MCP 截获，仍只为类型对齐留着"
    )]
    pub compactions: &'a mut CompactSlots,
    #[allow(
        dead_code,
        reason = "147 范围不含 Effect::Compact/MCP 截获，仍只为类型对齐留着"
    )]
    pub bus: &'a IoBus,
    pub agent: &'a AgentId,
    pub call_id: ToolCallId,
    /// 收窄前的原始入参。**`&Arc<Value>` 不是 `&Value`**（147 对 146 类型的
    /// 唯一一处调整，issue 原文预留了这个口子：「若装不进 146 的签名，回 146
    /// 改签名再来」）：既有四条截获跟 `session_tool_ext::adapt` 都要
    /// `Arc::clone` 它去造 `ctx.tools.snapshot` 的请求快照，narrow 成 `&Value`
    /// 会丢掉这个能力。
    pub input: &'a Arc<Value>,
    pub epoch: Epoch,
}

/// 内部层：一条截获的执行体。`Send + Sync`——actor 线程持有 `ctx`，注册进来的
/// 闭包要能跟着它一起被移到那条线上。
pub(crate) type InterceptFn = Box<dyn Fn(InterceptArgs<'_>) -> Dispatched + Send + Sync>;

/// builder 期逐个注册、会话期不再变的截获表。整张表包一层 `Arc`，理由与用法见
/// 模块文档「自借用陷阱」。
#[derive(Default)]
pub(crate) struct SessionToolRegistry {
    table: Arc<BTreeMap<Arc<str>, InterceptFn>>,
}

impl SessionToolRegistry {
    pub(crate) fn contains(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    /// 装配期原地插入。`Arc::get_mut` 要求「当前唯一持有者」——建表期间从没有
    /// 人 `Arc::clone` 过它，这个前提恒成立；真炸了说明有代码在会话运行期间
    /// （已经有一份 [`SessionToolRegistry::snapshot`] 克隆在飞）又调了一次注册，
    /// 那是违反「builder 期注册、会话期不变」契约的编程错误，值得响亮地失败
    /// ——不是需要在生产环境静默容忍的外部数据错误（`push_spec` 那类
    /// `debug_assert!` 兜的是后者，跟这里不是同一类问题）。
    pub(crate) fn register(&mut self, name: Arc<str>, f: InterceptFn) {
        let table = Arc::get_mut(&mut self.table).expect(
            "SessionToolRegistry::register 只能在装配期调用（此时是唯一持有者）；\
             若这里 panic，说明会话运行期间又调了一次注册",
        );
        table.insert(name, f);
    }

    /// 断开自借用的那一步：克隆出一份独立句柄，后续查找/调用都借这份克隆，跟
    /// `ctx`（进而 `self`）不再有任何借用关系。
    fn snapshot(&self) -> Arc<BTreeMap<Arc<str>, InterceptFn>> {
        Arc::clone(&self.table)
    }
}

impl RunnerCtx {
    /// [`RunnerCtx::register_intercept`]（本文件）与 `RunnerCtx::
    /// register_session_tool`（[`crate::session_tool_ext`]）共用的三道闸，判据
    /// 见模块文档「撞名判据」。
    pub(crate) fn registrable(&self, name: &str) -> Result<(), &'static str> {
        if self.tools.declares_timed(name) {
            return Err(
                "这个名字撞在 timed 区——timed 工具有自己的驱动（SessionStart/TurnEnd \
                 直接调 TimedTool::run，从不经过 ExecuteTool），不该再被这张表抢一次 \
                 执行权",
            );
        }
        if !self.tools.declares(name) {
            return Err(
                "这个名字没有在工具表里 declares()——spec 是声明、截获是执行路径，\
                 一名一路缺一不可",
            );
        }
        if self.session_tools.contains(name) {
            return Err("这个名字已经注册过截获式工具了，同名重复注册整条丢弃");
        }
        Ok(())
    }

    /// 147：内部层注册——迁移既有四条截获用。撞名判据见
    /// [`RunnerCtx::registrable`]；插进表的是**原始** `InterceptFn`，不经过
    /// `session_tool_ext::adapt`：调用方（`crate::builtin_intercepts`）保证这些
    /// 闭包已经自己管好可见性与屏障登记，包一层反而会重复（模块文档
    /// 「`dispatch` 不做任何包装」）。
    pub(crate) fn register_intercept(&mut self, name: Arc<str>, f: InterceptFn) {
        if let Err(reason) = self.registrable(&name) {
            debug_assert!(false, "register_intercept(`{name}`) 被拒：{reason}");
            return;
        }
        self.session_tools.register(name, f);
    }

    /// dispatch 用：这个名字有没有登记截获式扩展工具。已注册 ⟺ 已经在
    /// `declares()`（两个注册函数的前两道闸保证），所以命中即可直接截获，不用
    /// 像迁移前那样在调用点再查一遍 `declares()`。
    pub(crate) fn session_tool_registered(&self, name: &str) -> bool {
        self.session_tools.contains(name)
    }
}

/// dispatch 命中之后的查表 + 调用。**不做任何包装**（模块文档「`dispatch` 不做
/// 任何包装」）：两条注册路径各自在自己那一层还清了可见性/屏障的账，这里只剩
/// 「查表 + 调用」。
///
/// `tool`/`input` 是**收窄之前**的原始形态（`&str` + `Arc<Value>`）；
/// [`InterceptArgs::input`] 从这里的局部变量借出去。
// 10 个入参。**不合并成 struct**：这些是 dispatch 那一刻从调用方各处借出来的
// 引用（`&mut Session` / `&mut RunnerCtx` / `&mut Subtree` / `&mut CompactSlots`
// 四个可变借用来自四个不同的所有者），装进一个结构体要么要求它们同源、要么给这个
// 结构体加一堆生命周期参数，两种都比现在难读。参数多是这个函数「查表 + 调用、
// 不做任何包装」定位的直接后果，不是设计漏了一层。
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    compactions: &mut CompactSlots,
    bus: &IoBus,
    agent: &AgentId,
    call_id: ToolCallId,
    tool: &str,
    input: Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // 自借用陷阱在这里断开：`table` 是从 `ctx.session_tools` 克隆出来的独立
    // 句柄，`f` 借用的生命周期锚定在它身上，不是 `ctx`——所以下面把 `ctx` 塞进
    // `InterceptArgs` 再借一次 `&mut` 完全合法（模块文档「自借用陷阱」）。
    let table = ctx.session_tools.snapshot();
    let f = table
        .get(tool)
        .expect("调用点（crate::dispatch）已经用 session_tool_registered 确认过命中");
    f(InterceptArgs {
        session,
        ctx,
        subtree,
        compactions,
        bus,
        agent,
        call_id,
        input: &input,
        epoch,
    })
}
