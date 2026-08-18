//! 201：**带着钩子表调一次 undo**——三个宿主（CLI / server / 浏览器）唯一该调的
//! 撤销入口。
//!
//! 账本本身（谁登记、按什么键、什么时候丢）住 [`crate::undo_hook`]，这个文件只回答
//! 一句话：`Session` 的 `*_with` 要一个 `&mut dyn FnMut(&AgentEntry) -> HookOutcome`，
//! 那个回调长什么样。
//!
//! # 为什么是 runtime 出这三个函数，而不是每个宿主自己写回调
//!
//! 回调体只有一行（按 `entry.seq` 查表跑），但**写错的方式不止一种**：查不到时
//! 返回 `Ok` 而不是 `Lost`（= 恢复之后静默跳过一次真实副作用，199 §九 点名的
//! 那个静默错值）、或者把 `redo` 也接上钩子（redo 不重放副作用，200 §5）。
//! 三个宿主各写一遍就是三处可以各错一次，而这类错**测试未必红**——它只在
//! 「进程重启之后 undo 一条 `Hooked` 的 entry」这条罕见路径上浮出来。
//!
//! # `redo` 没有对应的函数，这是**故意**的
//!
//! redo 只是把值写回状态，不重放外部副作用（`Session::redo_turn` 的文档 / 200 §5）。
//! 所以宿主的 `redo` 照旧直接调 `session.redo_turn()`——这里不提供一个
//! `redo_turn_with_hooks`，免得有人以为「对称起见」该给它也接一个。

use agent_core::{AgentEntry, HookOutcome, Session, UndoReport};

use crate::ctx::RunnerCtx;

/// `Session` 上三个 `*_with` 的共同形状：收一个还原钩子回调，产出一份报告。
///
/// 起个别名只是因为写平了 clippy 的 `type_complexity` 会红——[`with_hooks`] 拿它
/// 当参数，好让三个入口共用同一句「先清表、再递回调」，而不是各抄一遍。
type UndoWith = fn(&mut Session, &mut dyn FnMut(&AgentEntry) -> HookOutcome) -> UndoReport;

/// `/undo`：撤一整轮，路上逐条跑还原钩子（决策 199 §三：钩子先跑，`Ok` 了才回滚
/// 这一条的状态）。
///
/// 撞上屏障、钩子跑挂了、钩子随进程重启没了——三种都返回
/// [`UndoReport::Blocked`]，`cause` 说明是哪一种（`agent_core::BlockedCause`）。
pub fn undo_turn(session: &mut Session, ctx: &mut RunnerCtx) -> UndoReport {
    with_hooks(ctx, session, Session::undo_turn_with)
}

/// `/undo!`：越过**第一条**障碍再退。「第一条」不是「全部」——一次确认只放行一个
/// 障碍（`Session::undo_turn_force_with` 的文档）。
pub fn undo_turn_force(session: &mut Session, ctx: &mut RunnerCtx) -> UndoReport {
    with_hooks(ctx, session, Session::undo_turn_force_with)
}

/// 退**一条** entry（开发者档 / 可展开时间线）。钩子语义与 [`undo_turn`] 相同。
pub fn undo_step(session: &mut Session, ctx: &mut RunnerCtx) -> UndoReport {
    with_hooks(ctx, session, Session::undo_step_with)
}

/// 三个入口共用的一句：先清掉已经被日志 cap 挤出去的钩子，再把「按 `seq` 查表跑」
/// 这个回调递给 core。
///
/// 借用之所以成立：`hooks` 借的是 `ctx` 的一个字段，`session` 是另一个所有者，
/// 两个可变借用互不相干。
fn with_hooks(ctx: &mut RunnerCtx, session: &mut Session, undo: UndoWith) -> UndoReport {
    ctx.undo_hooks.prune(session);
    let hooks = &mut ctx.undo_hooks;
    undo(session, &mut |entry| hooks.run(entry.seq))
}
