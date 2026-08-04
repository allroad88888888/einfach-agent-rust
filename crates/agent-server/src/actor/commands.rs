//! 每个 [`crate::command::Command`] 变体怎么落到 `Session` + `RunnerCtx` 上——
//! 驱动手法照抄 `agent-cli` 的 `repl.rs`/`undo.rs`（issue 030 原文指名参考的
//! 「repl/undo 的驱动手法」），只是把「打印到 stdout」换成「广播一条
//! [`SessionEvent`]」。
//!
//! **取消轮结束后的自动擦除**（[`erase_cancelled_turn`]）是 027 已经裁决过的
//! CLI 策略（`agent_cli::undo::after_cancelled_turn` 的同一条判断：非 force 的
//! `undo_turn`，`Applied` 就是干净擦除，`Blocked` 就是「这一轮已经执行过不可逆
//! 工具，保留」）。这里没有直接调用那个函数——`agent-server` 不依赖 `agent-cli`
//! （依赖方向见 `crate::provider_dispatch` 模块文档），策略本身只有三行，
//! 照抄比新引一条跨 crate 依赖更小。

use tokio::sync::broadcast::Sender as BroadcastSender;

use agent_core::{AgentId, Failure, Session, ToolCallId, TurnStatus};
use agent_runtime::{RemoteToolOutput, RunnerCtx, cancel_pending_remote_tools, resolve_remote_tool, run_turn};

use crate::command::Granularity;
use crate::event::{Frame, SessionEvent, UndoOutcome};

// 类型别名只是让下面的函数签名短一点。034：广播载荷是 `Frame`（agent 归属
// 信封），不再是裸的 `SessionEvent`。
type Events = BroadcastSender<Frame>;

/// 广播一条 `/undo` `/redo` 家族的结果——这几条命令是会话级的（不针对树上某
/// 个具体 agent），一律标 [`AgentId::root`]（034：`crate::event::frame` 模块
/// 文档同一条判据）。
fn emit_root(events: &Events, event: SessionEvent) {
    let _ = events.send(Frame { agent: AgentId::root(), event });
}

/// `Command::Input`：喂一句用户输入，跑一整轮。
///
/// 是否先 `begin_turn` 的判断，与取消轮结束后要不要自动擦除，跟 `agent_cli::
/// repl::run` 逐行一致——`session.status().is_terminal()` 才开新一轮，
/// `run_turn` 之后不管落在哪个终态都无条件调用一次（非终态卡住的情况交给
/// 调用方后续发 `Undo`/新的 `Input` 自行处理，跟 CLI 用户按 `/undo` 或者继续
/// 输入是同一个逃生舱，见 `repl.rs` 模块文档）。
pub(super) fn handle_input(session: &mut Session, ctx: &mut RunnerCtx, events: &Events, text: &str) {
    if session.status().is_terminal() {
        session.begin_turn();
        agent_runtime::persist::sync(ctx, session);
    }
    let status = run_turn(session, ctx, text);
    if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        erase_cancelled_turn(session, ctx, events);
    }
}

/// `Command::Undo { granularity, force }`：`Turn` + `force = false` 对应
/// `/undo`（撞屏障就停），`Turn` + `force = true` 对应 `/undo!`（越过第一条
/// 屏障）。
///
/// **`Step` 忽略 `force`**：`agent_core::Session` 没有 `undo_step` 的 force 变体
/// （`undo.rs` 模块文档——只有 turn 档设计了「越过第一条屏障」这个用户确认动作），
/// 031 的 HTTP 层在进队列前就已经拒绝 `granularity: "step", force: true` 这个
/// 组合（400，见 `crate::http::routes::undo`）——这里的 `_ => session.undo_step()`
/// 是防御性第二道闸：万一有调用方绕过 HTTP 层直接构造这个 `Command`（比如未来
/// 的另一种传输、或者测试），也不会把这个字段吞成「什么都不做」，而是做一件
/// 明确定义的事（忽略 force，退一条 entry）。
pub(super) fn handle_undo(session: &mut Session, ctx: &mut RunnerCtx, events: &Events, granularity: Granularity, force: bool) {
    ctx.discard_remote_tools();
    let report = match (granularity, force) {
        (Granularity::Turn, false) => session.undo_turn(),
        (Granularity::Turn, true) => session.undo_turn_force(),
        (Granularity::Step, _) => session.undo_step(),
    };
    agent_runtime::persist::sync(ctx, session);
    // 034：`from_report` 现查 `session` 富化 `Blocked`（工具名/call_id），不再是
    // 裸的 `UndoReport` 字段翻译——`session` 此刻还没被这次 undo 之外的任何东西
    // 改动过，barrier entry 就在它的 history 里。
    emit_root(events, SessionEvent::Undo(UndoOutcome::from_report(report, session)));
    // 048 补漏：undo 撤掉的子树也要让活树面板 / `GET .../agents` 看到——`handle_undo`
    // 不经 `run_turn` 的 pump，得在这里显式发一次树快照（真机验收逮到的漏投影：
    // core 层 `agent_tree()` 退了，SSE/GET 那一路没跟上）。
    ctx.emit_tree_snapshot(session);
}

/// `Command::Redo`。redo 没有屏障（`Session::redo_turn` 的文档：只是把值写
/// 回去，不重放外部副作用），结果只会是 `Applied`/`Nothing`，但这里不对
/// `UndoOutcome::Blocked` 做穷举排除——`UndoOutcome` 是个诚实的镜像类型，
/// 不该为了「redo 理论上到不了这个分支」而挖一个 `unreachable!`。
pub(super) fn handle_redo(session: &mut Session, ctx: &mut RunnerCtx, events: &Events) {
    ctx.discard_remote_tools();
    let report = session.redo_turn();
    agent_runtime::persist::sync(ctx, session);
    emit_root(events, SessionEvent::Redo(UndoOutcome::from_report(report, session)));
    // 048 补漏：redo 把子树接回来同样要让面板/GET 看到（见 handle_undo 同款注释）。
    ctx.emit_tree_snapshot(session);
}

/// `Command::Cancel`：Web 工具在等待槽位时 actor 会空闲地等待队列，因此除了
/// 立即翻转的原子标志，还要恢复一次事件泵使取消真正落入 session 历史。
pub(super) fn handle_cancel(session: &mut Session, ctx: &mut RunnerCtx, events: &Events) {
    if session.status().is_terminal() {
        return;
    }
    let status = cancel_pending_remote_tools(session, ctx);
    if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        erase_cancelled_turn(session, ctx, events);
    }
}

/// 远端回传先由 runtime 按 `(agent, call_id)` 消费等待槽，再恢复事件泵。无效或
/// 迟到结果只广播传输故障，绝不会写进当前工具调用。
pub(super) fn handle_remote_tool_result(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    events: &Events,
    agent: AgentId,
    call_id: ToolCallId,
    content: String,
    is_error: bool,
) {
    let output = if is_error { RemoteToolOutput::Failure(content) } else { RemoteToolOutput::Success(content) };
    match resolve_remote_tool(session, ctx, agent, call_id, output) {
        Ok(TurnStatus::Failed(Failure::Cancelled)) => erase_cancelled_turn(session, ctx, events),
        Ok(_) => {}
        Err(error) => emit_root(events, SessionEvent::TransportTrouble(std::sync::Arc::from(error.to_string()))),
    }
}

/// 远端等待到点了（060）：让 runtime 把过期的槽翻成 `is_error` 的工具结果并恢复
/// 事件泵。**不是**一条 `Command`——它没有来源，是「等命令等超了」这件事本身
/// （`super::body::next_command`）。
///
/// `None`（这一刻其实没有槽过期）就什么都不做。跟 `handle_input` 同款收尾：轮次
/// 万一落在 `Failed(Cancelled)`（比如超时恢复的那一圈里用户正好按了取消）照样走
/// 自动擦除，不为超时新造一条策略。
pub(super) fn handle_remote_tool_timeout(session: &mut Session, ctx: &mut RunnerCtx, events: &Events) {
    let Some(status) = agent_runtime::sweep_remote_tool_deadlines(session, ctx) else { return };
    if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        erase_cancelled_turn(session, ctx, events);
    }
}

/// 取消轮结束时的自动策略（027 已裁决，本文件模块文档）：非 force 的
/// `undo_turn`。结果照样走 [`SessionEvent::Undo`]——对客户端来说，「这一轮被
/// 取消后自动擦除」和「用户主动 `/undo`」产出的是同一种事件，语义也确实相同
/// （都是一次 `undo_turn`），不必另开变体。
fn erase_cancelled_turn(session: &mut Session, ctx: &mut RunnerCtx, events: &Events) {
    let report = session.undo_turn();
    agent_runtime::persist::sync(ctx, session);
    emit_root(events, SessionEvent::Undo(UndoOutcome::from_report(report, session)));
    // 048 补漏：取消轮自动擦除也撤子树，同样发一次树快照（见 handle_undo 同款注释）。
    ctx.emit_tree_snapshot(session);
}
