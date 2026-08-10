//! 105 独立测试：红线 6 在 `Effect::Compact` 的回执上落地——
//! `CancelInFlight` 之后带旧 epoch 回来的 `CompactDone` / `CompactFailed` 必须被
//! `Session::step` 的闸丢弃（不写状态、不 panic、不报错），而 epoch 对得上的同一
//! 条事件不该被同一把闸误伤（反向锁）。
//!
//! 场景造法照抄 `session_epoch_gate.rs` 的 `cancelled_session()`：`Event::Cancel`
//! 是 M1 就有的、会真的产出 `Effect::CancelInFlight` 并 bump 世代的那个动作
//! （「在飞 → 取消 → 迟到的结果回来」）。
//!
//! `CompactFailed` 是**正常事件不是异常路径**（106 验收原文）：这里额外测「收到
//! 它之后会话还活着，下一轮能继续」，不是把它当成失败去追杀。
//!
//! 全程零网络：不涉及任何真实 provider/工具调用，`Session::step` 是纯状态机，
//! `CompactDone`/`CompactFailed` 都是手工构造的事件。

use std::sync::Arc;

use agent_core::{Effect, Epoch, Event, Session, TurnStatus};

use crate::support;
use crate::support::session::{new_session, observe, session_with_pending_tools, thinking_session};

/// 一个已经 `Cancel` 过的会话——`Event::Cancel` 是 M1 就有的、真的会产出
/// `Effect::CancelInFlight` 并 bump 世代的动作，外加它 bump 之前的旧 epoch。
fn cancelled_session() -> (Session, Epoch) {
    let mut s = new_session();
    let old = s.epoch();
    let effects = s.step(support::cancel_event());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { .. })),
        "Cancel 必须发出 CancelInFlight，这是本场景的前提：{effects:?}"
    );
    assert_eq!(s.epoch(), old.next(), "Cancel 必须 bump 世代（红线 6）");
    (s, old)
}

/// `CancelInFlight` 之后，带旧 epoch 回来的 `CompactDone` 被丢弃：
/// 空 effects，状态一个字节不动，不 panic、不报错。
#[test]
fn stale_epoch_compact_done_after_cancel_in_flight_is_dropped() {
    let (mut s, old_epoch) = cancelled_session();
    let before = observe(&s);

    let effects = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: Arc::from("迟到的摘要，属于一个已经被取消掉的世界"),
        epoch: old_epoch,
    });

    assert_eq!(
        effects,
        Vec::<Effect>::new(),
        "旧 epoch 的 CompactDone 不该产出任何 effect"
    );
    assert_eq!(
        observe(&s),
        before,
        "旧 epoch 的 CompactDone 不该改动任何状态"
    );
}

/// 同样地，带旧 epoch 回来的 `CompactFailed` 也被丢弃——两个事件都要测，
/// 只测 `CompactDone` 挡不住一个只给它开了闸、给 `CompactFailed` 漏开的实现。
#[test]
fn stale_epoch_compact_failed_after_cancel_in_flight_is_dropped() {
    let (mut s, old_epoch) = cancelled_session();
    let before = observe(&s);

    let effects = s.step(Event::CompactFailed {
        agent: support::agent(),
        epoch: old_epoch,
    });

    assert_eq!(
        effects,
        Vec::<Effect>::new(),
        "旧 epoch 的 CompactFailed 不该产出任何 effect"
    );
    assert_eq!(
        observe(&s),
        before,
        "旧 epoch 的 CompactFailed 不该改动任何状态"
    );
}

/// 反向锁：只测「旧的被丢」挡不住一个「Compact 的回执不管 epoch 一律丢弃」的
/// 实现——那样的实现在上面两个测试里也会全绿。这里在**同一个会话**上先钉住
/// 旧 epoch 确实被丢（丢弃签名 = 空 effects），再把同一条 `CompactDone` 换成
/// 当前世代喂进去：如果闸真的比较过 epoch，结果就不该再是那个丢弃签名。
#[test]
fn epoch_matched_compact_done_is_not_silently_dropped() {
    // 用 undo（而不是 Cancel）bump 世代：undo 之后落回 `Idle`——一个仍在正常运转的
    // 状态，不是 Cancel 落地的那个终态。终态本身可能自带一条「任何事件都别管」的
    // 兜底，会话在世的时候拿到一条对不上号的 CompactDone 才是这条反向锁真正要防
    // 的场景（否则一个「终态一律不响应」的实现会假装闸生效了）。
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let old_epoch = s.epoch();
    let _ = s.undo_turn();
    assert_eq!(s.status(), TurnStatus::Idle);
    let current_epoch = s.epoch();
    assert_ne!(current_epoch, old_epoch, "undo 必须真的 bump 过世代，这是本反向锁的前提");

    // 基线：旧 epoch 在这个会话上确实被丢——跟上面两个测试同一断言，重新钉一遍
    // 是为了保证下面的对比是「同一起点」上的对比，不是两个不同会话的巧合。
    let before = observe(&s);
    let dropped = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: Arc::from("旧世代的摘要"),
        epoch: old_epoch,
    });
    assert_eq!(dropped, Vec::<Effect>::new());
    assert_eq!(observe(&s), before);

    // 同一条事件，只换成当前世代：这次必须过闸——效果不能跟上面那条一样是
    // 「空 effects」，否则分不清是真的过闸了还是 Compact 的回执被无条件吞掉。
    let effects = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: Arc::from("当前世代的摘要"),
        epoch: current_epoch,
    });

    assert_ne!(
        effects,
        Vec::<Effect>::new(),
        "epoch 对得上的 CompactDone 不该跟过期事件产出同一个『空 effects』签名——\
         那样就分不清是真的过闸了还是 Compact 事件被无条件丢弃：{effects:?}"
    );
}

/// `CompactFailed` 是正常事件不是异常路径（106 验收原文）：epoch 对得上的一条
/// `CompactFailed` 喂进一个正在 `Thinking` 的会话，不该把会话打进某种失败态——
/// 当前这一轮照常收尾，**下一轮**照常能开始、照常能调 provider。
#[test]
fn compact_failed_with_matching_epoch_leaves_the_session_alive_for_the_next_turn() {
    let mut s = thinking_session();
    let live_epoch = s.epoch();

    let _ = s.step(Event::CompactFailed {
        agent: support::agent(),
        epoch: live_epoch,
    });
    assert_eq!(
        s.status(),
        TurnStatus::Thinking,
        "压缩失败是背景维护动作，不该打断当前这一轮的状态"
    );

    // 当前这一轮照常收尾。
    let effects = s.step(support::provider_done_end_turn(s.epoch(), "答案"));
    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    assert!(
        !effects.is_empty(),
        "这一轮必须正常收尾并产出通报，不能因为刚才那条 CompactFailed 变哑"
    );

    // 下一轮：开新一轮、照常回到 Thinking 并发起下一次 CallProvider。
    s.begin_turn();
    assert_eq!(s.status(), TurnStatus::Idle);
    let effects = s.step(support::user_input_event("继续"));
    assert_eq!(s.status(), TurnStatus::Thinking, "下一轮必须能继续");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. })),
        "下一轮必须能正常发起 provider 调用：{effects:?}"
    );
}
