//! 026 等价重写自 `epoch_gate.rs`，外加 **026 验收「undo 后旧 epoch 的 `ToolResult`
//! 被丢弃」的端到端那一条**（红线 6 在原子图上的结账）。
//!
//! M1 用 `TurnState::bump_epoch()` 手动推世代；`Session` 没有那条后门——推世代的
//! 只有两个真实动作：**取消**和 **undo**。两条都在这里走一遍。

mod support;

use std::sync::Arc;

use agent_core::{
    Effect, Epoch, ErrorClass, Event, Session, StopReason, TokenUsage, TurnStatus,
};

use support::session::{new_session, observe, session_with_pending_tools};

/// 每种带 epoch 的事件变体各构造一份；`label` 用于失败信息。
fn epoch_bearing_event(label: &str, epoch: Epoch) -> Event {
    let agent = support::agent();
    match label {
        "ProviderDone" => Event::ProviderDone {
            agent,
            epoch,
            blocks: vec![],
            stop: StopReason::EndTurn,
            usage: TokenUsage { prompt: 1, completion: 1, cached: None },
            prefix: support::prefix_image(),
            adjustments: vec![],
        },
        "ProviderFailed" => Event::ProviderFailed {
            agent,
            epoch,
            class: ErrorClass::Retryable,
            message: Arc::from("boom"),
        },
        "ToolResult" => Event::ToolResult {
            agent,
            epoch,
            call_id: support::call_id(),
            content: Arc::from("ok"),
        },
        "ToolFailed" => Event::ToolFailed {
            agent,
            epoch,
            call_id: support::call_id(),
            error: Arc::from("nope"),
        },
        "Timeout" => Event::Timeout { agent, epoch, call_id: None },
        other => panic!("未知的测试标签：{other}"),
    }
}

const EPOCH_BEARING_LABELS: [&str; 5] = [
    "ProviderDone",
    "ProviderFailed",
    "ToolResult",
    "ToolFailed",
    "Timeout",
];

/// 一个已经 bump 过世代的会话（靠 `Cancel`——它是 M1 就有的那个 bump 者）。
fn cancelled_session() -> (Session, Epoch) {
    let mut s = new_session();
    let old = s.epoch();
    let _ = s.step(support::cancel_event());
    assert_eq!(s.epoch(), old.next());
    (s, old)
}

/// bump 之后用旧 epoch 喂：每种带 epoch 的事件都要被丢弃——空 `Vec`，
/// **状态一个字节不动，日志也不多一条**。
#[test]
fn stale_epoch_after_bump_is_dropped_for_every_epoch_bearing_event() {
    for label in EPOCH_BEARING_LABELS {
        let (mut s, old_epoch) = cancelled_session();
        let before = observe(&s);

        let effects = s.step(epoch_bearing_event(label, old_epoch));

        assert_eq!(
            effects,
            Vec::<Effect>::new(),
            "{label}: 过期（bump 之前的）epoch 不该产出任何 effect"
        );
        assert_eq!(observe(&s), before, "{label}: 过期事件不该改动状态");
    }
}

/// 未来的 epoch 同样被当作过期丢弃——闸判 `!=` 不判 `<`。
#[test]
fn future_epoch_is_also_dropped_for_every_epoch_bearing_event() {
    for label in EPOCH_BEARING_LABELS {
        let mut s = new_session();
        let future = s.epoch().next().next();
        let before = observe(&s);

        let effects = s.step(epoch_bearing_event(label, future));

        assert_eq!(effects, Vec::<Effect>::new(), "{label}: 未来 epoch 同样该被丢弃");
        assert_eq!(observe(&s), before, "{label}: 未来 epoch 事件不该改动状态");
    }
}

/// **026 验收：undo 之后旧 epoch 的 `ToolResult` 被丢弃**（红线 6 端到端）。
///
/// 场景就是这条红线存在的理由：两个工具在飞，用户按了 undo，其中一个的回执才姗姗
/// 来迟。它属于一个已经被回滚掉的世界，写进去就是一处「幽灵结果」——不报错、偶发、
/// 难复现。这里逐条钉住：世代真的推进了、旧回执一个字节都没写进去、
/// 而**同一条回执换上新世代就能正常落地**（证明挡住它的是闸不是别的什么）。
#[test]
fn a_tool_result_from_before_an_undo_is_dropped_but_the_same_one_lands_after_a_rewrite() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);
    let in_flight = s.epoch();

    // 用户 undo 掉这一整轮：状态退回开局，世代前进一格。
    let _ = s.undo_turn();
    assert_eq!(s.epoch(), in_flight.next(), "undo 必须 bump 世代（红线 6）");
    assert_eq!(s.status(), TurnStatus::Idle);
    let after_undo = observe(&s);

    // 迟到的回执带着旧世代：被闸挡掉，一个字节都不写。
    let effects = s.step(support::tool_result_event(in_flight, "call_1", "幽灵结果"));
    assert!(effects.is_empty());
    assert_eq!(observe(&s), after_undo, "旧世代的回执不该写进已经回滚掉的世界");

    // 同一条回执换成当前世代：这次过闸了（虽然状态对不上，判协议违规）——
    // 证明挡住上一条的确实是 epoch 闸，不是「Idle 收不到 ToolResult」这条。
    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "幽灵结果"));
    assert!(
        matches!(
            effects.as_slice(),
            [Effect::Emit(agent_core::Notice::ProtocolViolation { .. })]
        ),
        "{effects:?}"
    );
}

/// redo **不** bump 世代：undo 那一下已经把上一代的在飞 effect 全部作废了，
/// 把状态追回去不会让任何新的东西失效。
#[test]
fn redo_does_not_bump_the_epoch() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("hi"));
    let _ = s.undo_turn();
    let after_undo = s.epoch();

    let _ = s.redo_turn();

    assert_eq!(s.epoch(), after_undo);
    assert_eq!(s.status(), TurnStatus::Thinking, "状态确实被追回去了");
}

/// 用户意图不过闸：`UserInput` / `Cancel` 的 `Event::epoch()` 是 `None`，
/// 世代已经推进过也照样生效——用户永远针对当前世界说话。
#[test]
fn user_intent_never_goes_through_the_gate() {
    let (mut s, _) = cancelled_session();
    // 会话已经在 `Failed(Cancelled)`，先开新一轮回到 `Idle`。
    s.begin_turn();

    let effects = s.step(support::user_input_event("接着聊"));
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert!(effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })));

    let effects = s.step(Event::Cancel { agent: support::agent() });
    assert!(effects.iter().any(|e| matches!(e, Effect::CancelInFlight { .. })));
}
