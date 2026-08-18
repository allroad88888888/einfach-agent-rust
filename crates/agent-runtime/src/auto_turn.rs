//! 自驱动的轮次：**留言自己就能把下一轮启动**（211，决策 35 §二）。
//!
//! 这是本仓第一次让会话在**没有用户输入**的情况下继续消耗 token。所以这个文件
//! 一半是「让它跑起来」，另一半是「让它停得下来、看得见、喊得停」——后一半才是
//! 重点，而且写错了不报错。
//!
//! # 每一轮都是一次完整的新 turn，不是把一次 `run_turn` 拉长
//!
//! 循环在泵**外面**：`spend → begin_turn → drain_next_turn → 一次完整的泵`。
//! 于是 `turn_id` / undo 粒度 / 孤儿收尾**一个字都不用改**——每一轮自开的轮次
//! 在这三件事上跟一次用户输入长得一模一样。把它做成「一次超长的 `run_turn`」
//! 会把这三样全搅进来。
//!
//! # 停机：三条出路，每一条都要说话
//!
//! | 出路 | 判据 | 说的话 |
//! |---|---|---|
//! | 没有留言了 | root 收件箱里没有 `NextTurn` 条目 | 什么都不说（正常收工） |
//! | 预算见底 | [`Session::spend_auto_turn`] 回 `None` | `AutoTurnHeld { BudgetExhausted }` |
//! | 用户喊停（开轮之前） | 取消标志被置上 | `AutoTurnHeld { Cancelled }` |
//! | 用户喊停（这一轮跑到一半） | 这一轮落 `Failed(Cancelled)` | `undo_turn` 丢掉这半轮 → **留言退回收件箱** → `AutoTurnHeld { Cancelled }` |
//!
//! 后三条共有的承诺是**留言原地留着，不丢弃**。一个不说话的「什么都没发生」
//! 跟「留言被吞了」在外面长得一模一样，所以三条都必须 `emit` 出去。
//!
//! 最后那一条要动手才成立：留言在 `drain_next_turn` 那一刻已经**搬出**收件箱进了
//! `Messages`，所以「留在收件箱」靠的是把这半轮 `undo_turn` 掉。做在这里而不是
//! 让每个宿主自己做——211 的独立测试 agent 逮到的正是那个形状：浏览器宿主手工做
//! 了，CLI/server 走的 `run_auto_turns` 没做，同一条承诺在两个宿主上一个成立一个
//! 不成立，而且不报错。
//!
//! # 取消标志**不清**
//!
//! [`crate::runner_entry::run_turn_async`] 每轮开头清一次取消标志（那是用户按
//! 回车的语义：上一轮遗留的标志不该打断这一轮）。**自开的轮次没有那个语义**
//! ——没有人按回车，所以一个还没被处理的 Ctrl-C 必须仍然算数。这个循环因此走
//! [`crate::runner_entry::resume_async`]（不清标志），并且在开每一轮**之前**
//! 先看一眼标志。
//!
//! # 恢复不自开
//!
//! 这个文件不认识「恢复」——**恢复不自开是靠宿主不调这个函数**做到的
//! （211 §3）。宿主该调的是 [`report_recovered_mail`]：它只说一声「有 N 条留言
//! 等着，我不会自己去处理」，一轮都不开。
//!
//! 两条理由都不能让步：打开应用它自己就开始烧钱；以及用户还没来得及看上一轮
//! 发生了什么。**恢复是「回到现场」，不是「接着跑」。**

use std::sync::atomic::Ordering;

use agent_core::{Deliver, Event, Failure, Session, TurnStatus};

use crate::ctx::RunnerCtx;
use crate::event::{AutoTurnHold, RunnerEvent};
use crate::persist;
use crate::transient_source_failure::TransientSourceFailure;

/// root 收件箱里还有几条 `Deliver::NextTurn` 的留言。
///
/// **只数 `NextTurn`**：`Deliver::Now` 那一档是本轮的事（206 的排空定点 + 214 的
/// 唤醒），拿它当「该不该自开下一轮」的依据会把一条本该现在读的话推迟一整轮。
pub fn pending_next_turn_mail(session: &Session) -> usize {
    session
        .inbox_of(session.agent())
        .iter()
        .filter(|item| item.when == Deliver::NextTurn)
        .count()
}

/// 走一步：要么真的自开了一轮（返回它的终态），要么停住（原因已经报出去了）。
///
/// **单独暴露出来是给需要在每一轮之间插一脚的宿主用的**——浏览器宿主
/// （`agent-wasm`）在每一轮之后要排空 `web:` 工具的等待槽，那件事这个 crate
/// 不认识。`run_auto_turns_async` 就是这个函数的一个循环，没有别的东西。
pub async fn try_one_auto_turn_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<AutoTurnStep, TransientSourceFailure> {
    let root = session.agent().clone();
    let pending = pending_next_turn_mail(session);
    if pending == 0 {
        // 没有留言不是「停住」，是正常收工——不报任何东西。
        return Ok(AutoTurnStep::Idle);
    }
    // **先看取消，再花预算**：反过来的话，一次被取消的自开也会扣掉一格。
    if ctx.cancel_flag().load(Ordering::Relaxed) {
        hold(ctx, &root, pending, AutoTurnHold::Cancelled);
        return Ok(AutoTurnStep::Held);
    }
    // **减在开轮之前**：中间崩了的话，已经花掉的那一格不该复活。
    let Some(remaining) = session.spend_auto_turn() else {
        hold(ctx, &root, pending, AutoTurnHold::BudgetExhausted);
        return Ok(AutoTurnStep::Held);
    };
    ctx.emit(&root, RunnerEvent::AutoTurnStarted { remaining });

    // 一次完整的新 turn：turn 边界由 `begin_turn` 显式划（026 判断 13），
    // 留言在**新**这一轮里进历史（所以 `/undo` 掉这一轮，留言退回收件箱）。
    session.begin_turn();
    session.drain_next_turn();
    persist::sync(ctx, session);

    // `resume_async` 而不是 `run_turn_async`：没有用户那句话可喂，也**不能**
    // 清取消标志（见模块文档）。`Event::Wake` 让 root 用刚搬进 `Messages` 的
    // 那几条留言直接发一次请求。
    let status = crate::runner_entry::resume_async(
        session,
        ctx,
        Event::Wake {
            agent: root.clone(),
            epoch: session.epoch(),
        },
    )
    .await?;

    // 半路被取消：**这半轮要丢掉**（`undo_turn`，跟用户那一轮被取消时同一条路），
    // 于是那条 `drain_next_turn` 的 entry 跟着退掉，**留言退回收件箱**——这是
    // 「喊停之后剩下的留言留在收件箱、不丢弃」那条承诺的实际兑现处。
    //
    // 放在这里而不是让每个宿主自己做：211 的独立测试 agent 逮到的正是那个形状
    // ——浏览器宿主手工做了这件事，而 `run_auto_turns`（CLI/server 走的那条）
    // 没有，于是同一条承诺在两个宿主上一个成立一个不成立，还不报错。
    //
    // **预算不退**：provider 调用真的发出去了，钱烧掉了（跟 `/undo` 不退还它
    // 同一条理由，见 `agent_core::command::auto_turn` 模块文档）。
    if matches!(status, TurnStatus::Failed(Failure::Cancelled)) {
        let _ = crate::undo::undo_turn(session, ctx);
        persist::sync(ctx, session);
        hold(
            ctx,
            &root,
            pending_next_turn_mail(session),
            AutoTurnHold::Cancelled,
        );
        return Ok(AutoTurnStep::Held);
    }
    Ok(AutoTurnStep::Ran(status))
}

/// [`try_one_auto_turn_async`] 的三种去向。
///
/// 不 `derive(Eq)`：`TurnStatus` 自己没有（它装得下 `f32` 的用量数字那一支），
/// 而这个类型的用处是 `match` 分支，不是相等比较。
#[derive(Clone, PartialEq, Debug)]
pub enum AutoTurnStep {
    /// 真的自开了一轮，这是它的终态。
    Ran(TurnStatus),
    /// 没有留言等着——正常收工，什么都没报。
    Idle,
    /// 有留言但没开（预算见底 / 用户喊停）。**原因已经经
    /// [`RunnerEvent::AutoTurnHeld`] 报出去了**，调用方不必再说一遍。
    Held,
}

/// 一轮接一轮地自己跑，直到没有留言、预算见底、或者用户喊停。
///
/// 返回**每一轮的终态**，按发生顺序。空 `Vec` = 一轮都没自开。
///
/// 调用点在宿主处理完一次真实用户输入**之后**（`run_turn` 返回之后）。
pub async fn run_auto_turns_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<Vec<TurnStatus>, TransientSourceFailure> {
    let mut statuses = Vec::new();
    while let AutoTurnStep::Ran(status) = try_one_auto_turn_async(session, ctx).await? {
        statuses.push(status);
    }
    Ok(statuses)
}

/// [`run_auto_turns_async`] 的同步壳。**wasm 上没有**，理由同
/// [`crate::runner_entry::run_turn`]：`block_on` 靠停住当前线程来等，
/// 浏览器主线程一停，驱动 `fetch` 的事件循环跟着停 = 死锁。
#[cfg(not(target_arch = "wasm32"))]
pub fn run_auto_turns(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<Vec<TurnStatus>, TransientSourceFailure> {
    crate::block_on(run_auto_turns_async(session, ctx))
}

/// 刚恢复出来时说一声：**有留言等着，但我不会自己去处理**。
///
/// 宿主在 `recover` 之后调一次。没有留言就什么都不说。
///
/// 这个函数**一轮都不开**——「恢复不自开」不是靠它判断的，是靠宿主在恢复路径上
/// 调的是它而不是 [`run_auto_turns_async`]。分成两个函数正是为了让那条选择在
/// 调用点上看得见，而不是藏在一个 `if recovered` 里。
pub fn report_recovered_mail(session: &Session, ctx: &mut RunnerCtx) {
    let pending = pending_next_turn_mail(session);
    if pending > 0 {
        let root = session.agent().clone();
        hold(ctx, &root, pending, AutoTurnHold::Recovered);
    }
}

fn hold(ctx: &mut RunnerCtx, root: &agent_core::AgentId, pending: usize, reason: AutoTurnHold) {
    ctx.emit(root, RunnerEvent::AutoTurnHeld { pending, reason });
}
