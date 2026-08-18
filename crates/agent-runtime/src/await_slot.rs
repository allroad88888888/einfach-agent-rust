//! 等待槽：**谁的哪个工具槽在等谁**（212）。
//!
//! 跟 [`crate::subtree`] 的 `ChildSlots` 同构、同一条理由——一次 `await` 让调用方
//! 那个工具槽保持 `Pending`，泵每转一圈问一次「等到了没有」，到了就喂一条
//! `ToolResult` 回去。**登记住在运行时、判据住在 core**：
//!
//! - 「到了没有」是 `Session::await_progress`（读那个新 derived），
//! - 「谁在等谁」是 `Slot::AwaitingOn`（journaled，恢复之后还查得了环），
//! - 这里只有「哪个 call_id 在等」这一件**本轮内**的记账，跟 `ChildSlots` 一样
//!   每次 `resume` 重建。
//!
//! # 三条出路，一条都不能少
//!
//! | 目标 | 怎么收 |
//! |---|---|
//! | 到了 | `ToolResult`，正文说清「它到了，正文要 collect」 |
//! | 收场了但不是你等的那一种 | `ToolFailed`——**继续等就是永远等** |
//! | 已经不活着（被撤销/拆掉） | `ToolFailed`，同上 |
//!
//! 后两条是 212 §4 点名的那条防死等：**不能让槽永远 `Pending`**。泵的静止条件是
//! 「两张在飞表都空」，而一个挂着的 await 不占任何在飞表——它会安安静静地留下
//! 一个永远收敛不了的槽，没有 panic、没有超时、没有告警。

use std::sync::Arc;

use agent_core::{AgentId, AwaitProgress, AwaitUntil, Epoch, Event, Session, ToolCallId};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

/// 一次挂起的 `await`。
struct AwaitSlot {
    waiter: AgentId,
    target: AgentId,
    until: AwaitUntil,
    call_id: ToolCallId,
    /// 发起那一刻的世代（红线 6）：回写前比一次，不等就丢——跟 `ChildSlot.epoch`
    /// 同一条理由。`/undo` 期间目标的 `Status` 会变，那一下不该把一个已经作废的
    /// 槽写活。
    epoch: Epoch,
}

/// 本轮所有挂起的 `await`。
#[derive(Default)]
pub(crate) struct AwaitSlots {
    slots: Vec<AwaitSlot>,
}

impl AwaitSlots {
    /// 记一笔：`waiter` 的 `call_id` 那个槽在等 `target` 到达 `until`。
    pub(crate) fn record(
        &mut self,
        waiter: AgentId,
        target: AgentId,
        until: AwaitUntil,
        call_id: ToolCallId,
        epoch: Epoch,
    ) {
        self.slots.push(AwaitSlot {
            waiter,
            target,
            until,
            call_id,
            epoch,
        });
    }

    /// 泵每转一圈问一次：有等到的（或等不到的）就产出收敛事件，并把那一笔划掉。
    ///
    /// **同时清掉 core 里那条等待边**（`Session::stop_awaiting`）：不清的话，
    /// 等待图里会留下一条已经了结的边，而查环走的正是那张图——留着就等于把一个
    /// 本该放行的反向 `await` 永久挡掉。
    pub(crate) fn harvest(&mut self, session: &mut Session, ctx: &mut RunnerCtx) -> Vec<Event> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.slots.len() {
            let outcome = decide(session, &self.slots[i]);
            let Some((body, is_error)) = outcome else {
                i += 1;
                continue;
            };
            let slot = self.slots.remove(i);
            session.stop_awaiting(&slot.waiter, &slot.target);
            ctx.emit(
                &slot.waiter,
                RunnerEvent::ToolExecuted {
                    call_id: slot.call_id.clone(),
                    tool: Arc::from(crate::AWAIT_TOOL),
                    output_len: body.len(),
                    is_error,
                },
            );
            events.push(if is_error {
                Event::ToolFailed {
                    agent: slot.waiter,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    error: Arc::from(body),
                }
            } else {
                Event::ToolResult {
                    agent: slot.waiter,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    content: Arc::from(body),
                }
            });
        }
        events
    }

}

/// 这一笔该收了吗——`None` = 接着等。
///
/// 三条出路的判据都在这里，**不散落在调用点**：散开写就是三处各自可能漏判一条，
/// 而漏掉后两条中的任何一条，症状都是「一个永远 `Pending` 的槽」。
fn decide(session: &Session, slot: &AwaitSlot) -> Option<(String, bool)> {
    // 目标没了（被撤销 / 轮末被拆 / 从没 spawn 出来过）：等不到了。
    // **这一条要排在 `await_progress` 之前**——一个死掉的 agent 的 `Status` 槽位
    // 可能停在任何值上，拿它去判「到了没有」是在读一份没有意义的状态。
    if !session.is_live(&slot.target) {
        return Some((
            format!(
                "await 结束：{} 已经不在活 agent 里了（被撤销、或者这一轮收尾时被拆掉）。\
                 它不会再到达任何状态了，别再等。",
                slot.target.as_str(),
            ),
            true,
        ));
    }
    match session.await_progress(&slot.target, slot.until) {
        AwaitProgress::Waiting => None,
        AwaitProgress::Reached => Some((
            format!(
                "{} 到了（{}）。**这里不给正文**——要它的回答用 srv:agent/collect 领。",
                slot.target.as_str(),
                describe(slot.until),
            ),
            false,
        )),
        AwaitProgress::Unreachable => Some((
            format!(
                "await 结束：{} 已经收场了，但不是你等的那一种（你等的是{}）。\
                 继续等就是永远等，所以这里直接告诉你。要看它到底怎么收场的，\
                 用 srv:agent/status。",
                slot.target.as_str(),
                describe(slot.until),
            ),
            true,
        )),
    }
}

/// `until` 的人话。进 prompt，所以逐字节确定（红线 11）：一个 `match`，没有别的。
fn describe(until: AwaitUntil) -> &'static str {
    match until {
        AwaitUntil::Settled => "它收场（不管成没成）",
        AwaitUntil::Done => "它成功收场",
        AwaitUntil::Failed => "它失败收场",
    }
}
