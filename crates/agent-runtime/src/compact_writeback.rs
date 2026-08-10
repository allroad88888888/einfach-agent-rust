//! 摘要回写的 **epoch 握手**：107 留给 108 的那条硬契约，在这里变成一处显式、
//! 有名字、有测试的判定。
//!
//! # 契约原文（107 §「epoch 校验在哪」）
//!
//! > 持有 `upto` 的一方必须**先**把 `Event::CompactDone` 喂给 `Session::step`；
//! > **只有过了闸**——回执里出现 `Notice::CompactionSummaryReceived`——才调
//! > `apply_summary`。
//!
//! 为什么契约长这样，两句话：`apply_summary` 是一条**命令**（跟
//! `advance_boundary` / `clear_tool_results` 一样表达「此刻的意图」），签名里没有
//! epoch，红线 6 的闸只住在 `Session::step`；而 `Event::CompactDone` **不带
//! `upto`**（105 定死的事件形状，effect 不带历史正文，事件也没有理由胖），所以
//! `step` 自己也回写不了。于是回写必然是「先过闸、再由记着 `upto` 的一方写」这
//! 两步，而两步之间的那个判据守不进类型系统。
//!
//! # 为什么不写成「回执非空」
//!
//! 今天 `Event::CompactDone` 过闸之后恰好只产出一条 effect，所以
//! `!effects.is_empty()` 碰巧也对。但它对的是**巧合**，不是语义：
//!
//! - 哪天那一格多发一条通报（109 要做的可见性就在这一格上），「非空」还对；
//! - 哪天 `step` 对**没过闸**的回执也说一句（比如加一条「丢弃了一份过期摘要」的
//!   通报），「非空」当场变成永远为真——**一份属于旧世代的摘要会被写进当前状态**：
//!   边界推到 `upto`，而那段历史已经被 undo 掉了，下一轮 prompt 少一整段，模型照
//!   答不误，人发现不了。那正是红线 6 原话里「在 undo 或崩溃恢复时以静默错值的
//!   形式浮出来」的形态。
//!
//! [`passed_epoch_gate`] 判的是**那一条具体的通报在不在**，所以上面两种改动都不会
//! 让它悄悄换答案。它有专门的变异检验（见本文件测试）：一批非空但不含那条通报的
//! effect 必须判 `false`。

use std::sync::Arc;

use agent_core::{AgentId, Effect, Epoch, Notice, Session};

use crate::compact_slot::CompactSlots;
use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::persist;

/// 一份已经收割、正在等着过闸的摘要。
///
/// `upto` **只有这一侧记着**：它不在 `Event::CompactDone` 里（105 定死），也不在
/// `Session` 里（还没写进去）。这张记账因此是那条硬契约唯一的物理载体。
pub(crate) struct PendingSummary {
    /// 这次压缩归属的 agent（回执事件的 `agent`，不是摘要子 agent）。
    pub(crate) agent: AgentId,
    /// spawn 摘要子 agent 那一刻的世代。跟 `Session::epoch()` 比一次就知道这份
    /// 意图还算不算数——世代只增不减，对不上就永远对不上了。
    pub(crate) epoch: Epoch,
    /// 这次摘要盖住的边界。
    pub(crate) upto: usize,
    /// 摘要正文（红线 5：大值 `Arc`，这里只是指针拷贝）。
    pub(crate) summary: Arc<str>,
}

/// **过闸的判据**：`Session::step` 吃下 `Event::CompactDone` 之后，回执里出现
/// [`Notice::CompactionSummaryReceived`] 才算这份摘要属于当前世代、可以回写。
///
/// 见模块文档「为什么不写成『回执非空』」。
pub(crate) fn passed_epoch_gate(effects: &[Effect]) -> bool {
    effects
        .iter()
        .any(|effect| matches!(effect, Effect::Emit(Notice::CompactionSummaryReceived)))
}

/// 每次 `session.step(event)` 之后调一次：过闸就回写，没过闸就丢。
///
/// **不是只在 `CompactDone` 之后调**——调用点（泵的 A 段）拿不到已经被 `step`
/// 吃掉的那个事件，而「哪些事件该走这条路」正是 [`passed_epoch_gate`] 自己回答的
/// 问题。对其余事件它是一次 `Vec` 的空扫描。
///
/// 顺带做一件卫生：世代已经推走的回写意图当场丢掉。它们**不可能**再对上
/// （epoch 只增不减），留着只会让这张表在一轮里无界地长。
pub(crate) fn after_step(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    compactions: &mut CompactSlots,
    agent: &AgentId,
    effects: &[Effect],
) {
    let now = session.epoch();
    compactions.drop_stale_summaries(now);
    if !passed_epoch_gate(effects) {
        return;
    }
    let Some(pending) = compactions.take_gated_summary(agent, now) else {
        return;
    };
    let upto = pending.upto;
    match session.apply_summary(&pending.agent, pending.upto, pending.summary) {
        // 三件事（存正文 / 推边界 / 填引用）已经落成一条 entry，跟别的命令一样立刻
        // 转发进持久化后端。`Ok` 也可能是一次幂等无操作（没有新 entry），那时这次
        // 转发是空操作——`persist::sync` 按 seq 高水位判，不会重发任何东西。
        //
        // 109：压缩点在时间线上可见的信号。跟着发是因为这一刻状态已经写完了
        // （`Slot::Summaries` + `Slot::SendPlan` 同一条 entry），`upto` 也只有
        // 这一侧记着（模块文档「`upto` 只有这一侧记着」）——过这一刻再想要就
        // 没有别的地方能补。幂等无操作那一支（没有新 entry）也照发：宿主重放
        // 一份逐字相同的摘要不该在可见性上跟第一次发生有区别，UI 拿它当「压缩点
        // 仍然存在」的确认没有坏处。
        Ok(id) => {
            persist::sync(ctx, session);
            ctx.emit(
                agent,
                RunnerEvent::CompactionApplied {
                    turn_id: session.turn_id(),
                    upto,
                    summary_id: id,
                },
            );
        }
        // 边界语义上的拒绝（例如第 4 档刚清过窗口，一份正好盖到这个边界的摘要迟到
        // 了）。状态与日志都没动（107：拒绝路径不留痕），但**不静默**：这一次压缩
        // 确实没成，报的就是它——不新造 Notice 变体，`CompactionFailed` 说的正是
        // 这件事。
        Err(_rejected) => ctx.emit(agent, RunnerEvent::Notice(Notice::CompactionFailed)),
    }
}

#[cfg(test)]
mod tests {
    use agent_core::TurnStatus;

    use super::*;

    /// 过闸那一条通报在场 → `true`。
    #[test]
    fn the_acceptance_notice_is_what_opens_the_gate() {
        let effects = vec![Effect::Emit(Notice::CompactionSummaryReceived)];
        assert!(passed_epoch_gate(&effects));
    }

    /// 被闸挡下的回执：`step` 返回空 `Vec`（不写、不报错、不通报）→ `false`。
    #[test]
    fn an_empty_receipt_never_opens_the_gate() {
        assert!(!passed_epoch_gate(&[]));
    }

    /// **变异检验**：这批 effect 非空，但没有那一条通报。写成「回执非空 = 过闸」
    /// 的实现在这里判 `true`，于是一份过期的摘要会被写进状态——红线 6 的静默错值。
    #[test]
    fn a_non_empty_receipt_without_the_notice_does_not_open_the_gate() {
        let effects = vec![
            Effect::Emit(Notice::CompactionFailed),
            Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::Done { truncated: false },
            }),
            Effect::Emit(Notice::ProtocolViolation {
                state: TurnStatus::Idle,
                event: Arc::from("whatever"),
            }),
        ];
        assert!(
            !passed_epoch_gate(&effects),
            "非空 ≠ 过闸：判据是那一条具体的通报在不在"
        );
    }

    /// 一批 effect 里混着那条通报也算过闸——判的是「在不在」，不是「是不是唯一
    /// 一条」（109 给这一格加通报时不该让这条判定换答案）。
    #[test]
    fn the_notice_is_found_anywhere_in_the_batch() {
        let effects = vec![
            Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::Idle,
            }),
            Effect::Emit(Notice::CompactionSummaryReceived),
        ];
        assert!(passed_epoch_gate(&effects));
    }
}
