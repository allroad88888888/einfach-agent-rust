//! 147：内置工具截获迁进 [`crate::intercept_registry`] 的表。
//!
//! 迁的时候是四条（spawn/collect/status/skill-read），现在是**八条**——
//! 206 的 `send`、208 的 `self`、209 的 `notes` 读写两条各自追加了一行。
//!
//! # 这是「哪些内置工具用这张表」，不是「这张表怎么工作」
//!
//! 机制本身（`InterceptArgs`/`InterceptFn`/自借用陷阱/撞名判据）住
//! `intercept_registry.rs`——那是通用的装配期正门，不认识任何具体工具。这个
//! 文件反过来：只认识那几个具体工具，不管表怎么实现。拆开是「一个文件一件事」
//! （红线 9）：注册表的自借用/撞名/公开签名已经够那个文件讲一整篇，再塞进
//! 「这些工具具体转发给谁」会让两件不同颗粒度的事挤在一起。
//!
//! # 内部层直调，不走 `SessionToolFn`
//!
//! 这些截获要碰的东西够不着公开层的收窄签名（`SessionToolFn` 只留
//! `&mut Session` + `&AgentId` + `&Value`，见 `crate::session_tool_ext`）：
//! spawn/collect 要读写 `Subtree`，spawn 后台模式还要一次产出两个事件
//! （`Dispatched::Events`），公开层的 `Result<Arc<str>, Arc<str>>` 返回值装不下。
//! 所以这里直接构造**内部层** `InterceptFn`，插进跟 `SessionToolFn` 扩展共用的
//! 同一张表（[`RunnerCtx::register_intercept`]，绕开 `session_tool_ext::adapt`）。
//!
//! # 零包装：这些闭包已经自己管好可见性与屏障登记
//!
//! 迁移前 `dispatch.rs` 手工 if 链直接调用 `xxx_tool::intercept`，那几个函数
//! 内部自己 `ctx.tools.snapshot` + `ctx.emit(ToolExecuting)`（该不该
//! `mark_no_undo` 也是它们自己的事，虽然它们全是 `Pure`/`Reversible`，
//! 从没真的调用过）。下面每个闭包原样转发，不加一分钱包装——
//! `intercept_registry::dispatch`（通用查表+调用那一段）本身也不包装。跟
//! `session_tool_ext::adapt`（`SessionToolFn` 那条公开路径的适配器，
//! `ctx.emit`/屏障登记在那边补上，因为纯读闭包自己够不着 `ctx`）刻意不同：
//! 这些能拿到完整的 `InterceptArgs`，包一层反而会让 `ToolExecuting` 发两次
//! ——147 的验收就是揪住这个不许发生。
//!
//! # 注册点：`RunnerCtx::new` 的构造链
//!
//! [`register_builtin_intercepts`] 在那之后立刻调用，按 `declares()` 判断要不要
//! 注册——跟各自 `with_*`（`with_spawn`/`with_collect`/`with_status`/`with_send`/
//! `with_self`/`with_notes`/`with_skills`）让 `declares()` 为真的时机同一条件，
//! 声明与执行路径因此天然
//! 同开同关，cli/server/wasm 三个宿主零改动（它们都经同一个 `RunnerCtx::new`）。

use std::sync::Arc;

use crate::collect_tool::{self, COLLECT_TOOL};
use crate::ctx::RunnerCtx;
use crate::intercept_registry::{InterceptArgs, InterceptFn};
use crate::notes_tool::{self, NOTES_SET_TOOL, NOTES_TOOL};
use crate::self_tool::{self as self_tool, SELF_TOOL};
use crate::skill::{self, SKILL_READ};
use crate::spawn_tool::{self, SPAWN_TOOL};
use crate::send_tool::{self, SEND_TOOL};
use crate::status_tool::{self, STATUS_TOOL};

/// 八个内置截获，`declares()` 为真才注册（一名一路，见 `intercept_registry`
/// 模块文档「撞名判据」）。命中即 `debug_assert_eq!` 校验「声明⟺注册」——这是
/// 半开状态「表 declares 但没注册」那一半的看门狗：镜像的另一半（「注册了但
/// 表没 declares」）已经在 `RunnerCtx::registrable` 共用的三道闸里（146 的
/// `debug_assert!`），不在这里重复。
pub(crate) fn register_builtin_intercepts(ctx: &mut RunnerCtx) {
    let builtins: [(&str, InterceptFn); 8] = [
        (SPAWN_TOOL, spawn_intercept()),
        (COLLECT_TOOL, collect_intercept()),
        (STATUS_TOOL, status_intercept()),
        (SEND_TOOL, send_intercept()),
        (SELF_TOOL, self_intercept()),
        (NOTES_TOOL, notes_read_intercept()),
        (NOTES_SET_TOOL, notes_set_intercept()),
        (SKILL_READ, skill_read_intercept()),
    ];
    for (name, f) in builtins {
        if ctx.tools().declares(name) {
            ctx.register_intercept(Arc::from(name), f);
        }
        debug_assert_eq!(
            ctx.tools().declares(name),
            ctx.session_tool_registered(name),
            "`{name}` 的工具表声明与截获注册必须同开同关——半开状态是 146/147 \
             那道闸本该挡住的事"
        );
    }
}

fn spawn_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            subtree,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        spawn_tool::intercept(session, ctx, subtree, agent, call_id, input, epoch)
    })
}

fn collect_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            subtree,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        collect_tool::intercept(session, ctx, subtree, agent, call_id, input, epoch)
    })
}

fn status_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        status_tool::intercept(session, ctx, agent, call_id, input, epoch)
    })
}

fn send_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            subtree,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        send_tool::intercept(session, ctx, subtree, agent, call_id, input, epoch)
    })
}

fn self_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        self_tool::intercept(session, ctx, agent, call_id, input, epoch)
    })
}

fn notes_read_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        notes_tool::read_intercept(session, ctx, agent, call_id, input, epoch)
    })
}

fn notes_set_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            session,
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        notes_tool::set_intercept(session, ctx, agent, call_id, input, epoch)
    })
}

fn skill_read_intercept() -> InterceptFn {
    Box::new(|args: InterceptArgs<'_>| {
        let InterceptArgs {
            ctx,
            agent,
            call_id,
            input,
            epoch,
            ..
        } = args;
        skill::read_intercept(ctx, agent, call_id, input, epoch)
    })
}

#[cfg(test)]
#[path = "builtin_intercepts_tests.rs"]
mod tests;
