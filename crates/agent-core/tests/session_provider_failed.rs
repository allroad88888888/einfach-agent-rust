//! 026 等价重写自 `provider_error_classification.rs`：`Thinking + ProviderFailed`
//! 的内部子分支（016）——按 `ErrorClass` 分流、重试预算的消耗与耗尽、
//! `retries_used` 的清零时机。
//!
//! 唯一的形状变化：M1 直接给 `st.max_retries` 赋值，这里走
//! `Session::set_max_retries`（红线 2：预算也是 primitive，改它同样要留下 `Entry`）。

mod support;

use agent_core::{Effect, ErrorClass, Failure, Notice, TurnStatus};

use support::session::{new_session, thinking_session};

/// `BadRequest`/`Auth`/`Exhausted`/`Unknown` 四个非 `Retryable` 类：立刻
/// `Failed(Provider(class))`，重试预算完全不消耗——「重试的判断」压根没有走到
/// 「有没有预算」那一步。
#[test]
fn non_retryable_classes_fail_immediately_without_consuming_retry_budget() {
    for class in [
        ErrorClass::BadRequest,
        ErrorClass::Auth,
        ErrorClass::Exhausted,
        ErrorClass::Unknown,
    ] {
        let mut s = thinking_session();
        let effects = s.step(support::provider_failed_event_with_class(s.epoch(), class));

        let expected = TurnStatus::Failed(Failure::Provider(class));
        assert_eq!(s.status(), expected, "{class:?}");
        assert_eq!(s.retries_used(), 0, "{class:?}：没有重试，预算不该被消耗");
        assert_eq!(
            effects,
            vec![Effect::Emit(Notice::TurnStatusChanged { status: expected })],
            "{class:?}"
        );
    }
}

/// 验收原文点名：`Exhausted` 永不重试，就算重试预算异常充裕也不例外——混进限流
/// 重试会让系统安静地退避到天荒地老。
#[test]
fn exhausted_never_retries_even_with_ample_budget() {
    let mut s = thinking_session();
    s.set_max_retries(100);

    let effects = s.step(support::provider_failed_event_with_class(
        s.epoch(),
        ErrorClass::Exhausted,
    ));

    assert_eq!(
        s.status(),
        TurnStatus::Failed(Failure::Provider(ErrorClass::Exhausted))
    );
    assert_eq!(s.retries_used(), 0);
    assert!(!effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })));
}

/// `Retryable`：重试到预算耗尽为止，耗尽的那一次才落 `Failed`。
#[test]
fn retryable_retries_until_budget_exhausted_then_fails() {
    let mut s = thinking_session();
    s.set_max_retries(2);

    for expected_attempt in [1u32, 2] {
        let effects = s.step(support::provider_failed_event(s.epoch()));
        assert_eq!(
            s.status(),
            TurnStatus::Thinking,
            "第 {expected_attempt} 次重试还留在 Thinking"
        );
        assert_eq!(s.retries_used(), expected_attempt);
        assert_eq!(effects.len(), 2, "第 {expected_attempt} 次：Retrying 通报 + CallProvider");
        assert!(matches!(
            effects[0],
            Effect::Emit(Notice::Retrying { attempt, max_retries: 2 }) if attempt == expected_attempt
        ));
        assert!(matches!(effects[1], Effect::CallProvider { .. }));
    }

    // 第三次：预算耗尽，放弃。
    let effects = s.step(support::provider_failed_event(s.epoch()));
    let expected = TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable));
    assert_eq!(s.status(), expected);
    assert_eq!(s.retries_used(), 2, "耗尽之后不再继续增加");
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged { status: expected })]
    );
}

/// `ProviderDone` 成功之后，失败连续计数清零——下一次失败重新获得满额预算，
/// 不是在整轮总共重试次数上封顶。
#[test]
fn retries_used_resets_after_a_successful_provider_done() {
    let mut s = thinking_session();

    let _ = s.step(support::provider_failed_event(s.epoch()));
    assert_eq!(s.retries_used(), 1);

    let _ = s.step(support::provider_done_end_turn(s.epoch(), "ok"));
    assert_eq!(s.retries_used(), 0, "拿到成功响应之后，失败连续计数清零");
}

/// 重试预算充足，但 `max_turns` 已经顶格：`try_call_provider` 把这次「决定重试」
/// 兜底成 `Done{truncated:true}`，且**不该**假装真的发起了一次重试。
#[test]
fn retry_blocked_by_max_turns_falls_back_to_done_truncated() {
    let mut s = new_session();
    s.set_max_turns(1);

    // 先合法地用掉唯一的一次 CallProvider 预算。
    let _ = s.step(support::user_input_event("hi"));
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert_eq!(s.turns_used(), 1);

    let effects = s.step(support::provider_failed_event(s.epoch()));

    assert_eq!(s.status(), TurnStatus::Done { truncated: true });
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: true }
        })]
    );
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::Emit(Notice::Retrying { .. }))),
        "没有真的重试，不该报 Retrying"
    );
}
