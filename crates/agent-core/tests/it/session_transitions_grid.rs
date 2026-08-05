//! 026 等价重写自 `turn_transitions_grid.rs`：`TurnStatus`（5 态）× `Event`（7 变体）
//! = 35 格，**10 格合法 / 25 格非法**，没有隐式的「忽略」。
//!
//! 断言逐条对应 M1 那份，只有两处形状变化：
//!
//! 1. 造状态从「给字段赋值」变成「用事件驱动」——`Session` 不暴露 store，没有后门
//!    （见 `support/session.rs`）。
//! 2. 「非法转移不该改状态」从 `assert_eq!(st, before)` 变成
//!    `assert_eq!(observe(&s), before)`：所有 primitive 逐值 + epoch + turn_id
//!    + **日志没多出一条 entry**。最后一项是 M1 没有的那一半。

use crate::support;
use agent_core::{Effect, Notice, ToolCallId, TurnStatus};

use crate::support::session::{Observed, all_statuses, observe, session_at};

fn assert_violation(effects: &[Effect], expected_status: &TurnStatus) {
    assert_eq!(
        effects.len(),
        1,
        "非法组合应该只产出一条 ProtocolViolation：{effects:?}"
    );
    match &effects[0] {
        Effect::Emit(Notice::ProtocolViolation { state, .. }) => assert_eq!(state, expected_status),
        other => panic!("期待 ProtocolViolation，收到 {other:?}"),
    }
}

fn assert_untouched(before: &Observed, after: Observed, status: &TurnStatus) {
    assert_eq!(
        &after, before,
        "{status:?} 下的非法转移不该改状态，也不该在 undo 栈里留一步"
    );
}

/// `UserInput`：只有 `Idle` 合法。
#[test]
fn user_input_legal_only_from_idle() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::user_input_event("你好"));

        if matches!(status, TurnStatus::Idle) {
            assert_eq!(s.status(), TurnStatus::Thinking);
            assert_eq!(s.messages().len(), 1, "用户消息应该进历史");
            assert_eq!(effects.len(), 2);
            assert!(matches!(
                effects[0],
                Effect::Emit(Notice::TurnStatusChanged {
                    status: TurnStatus::Thinking
                })
            ));
            assert!(matches!(effects[1], Effect::CallProvider { .. }));
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `ProviderDone`：只有 `Thinking` 合法。
#[test]
fn provider_done_legal_only_from_thinking() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::provider_done_end_turn(s.epoch(), "答案"));

        if matches!(status, TurnStatus::Thinking) {
            assert_eq!(s.status(), TurnStatus::Done { truncated: false });
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `ToolResult`：只有 `ToolsPending` 合法（且 `call_id` 得对得上）。
#[test]
fn tool_result_legal_only_from_tools_pending() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "ok"));

        if matches!(status, TurnStatus::ToolsPending) {
            // 唯一的槽收敛了：状态应该已经推进回 Thinking。
            assert_eq!(s.status(), TurnStatus::Thinking);
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `ToolFailed`：只有 `ToolsPending` 合法，跟 `ToolResult` 同构（001 判断 3）。
#[test]
fn tool_failed_legal_only_from_tools_pending() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::tool_failed_event(s.epoch(), "call_1", "boom"));

        if matches!(status, TurnStatus::ToolsPending) {
            assert_eq!(s.status(), TurnStatus::Thinking);
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `ProviderFailed`：只有 `Thinking` 合法。固定 `ErrorClass::Retryable`、默认预算
/// 没耗尽，所以 `Thinking` 分支走的是「重试」而不是「放弃」。
#[test]
fn provider_failed_legal_only_from_thinking() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::provider_failed_event(s.epoch()));

        if matches!(status, TurnStatus::Thinking) {
            assert_eq!(
                s.status(),
                TurnStatus::Thinking,
                "重试预算没耗尽，留在 Thinking"
            );
            assert_eq!(s.retries_used(), 1);
            assert_eq!(effects.len(), 2, "Retrying 通报 + 重发的 CallProvider");
            assert!(matches!(
                effects[0],
                Effect::Emit(Notice::Retrying {
                    attempt: 1,
                    max_retries: 2
                })
            ));
            assert!(matches!(effects[1], Effect::CallProvider { .. }));
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `Timeout` 的 provider 支（`call_id: None`）：只在 `Thinking` 合法。
#[test]
fn timeout_provider_leg_legal_only_from_thinking() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::timeout_event(s.epoch(), None));

        if matches!(status, TurnStatus::Thinking) {
            assert_eq!(s.status(), TurnStatus::Thinking);
            assert_eq!(
                s.retries_used(),
                1,
                "provider 超时按 Retryable 走同一条重试路"
            );
            assert_eq!(effects.len(), 2);
            assert!(matches!(effects[1], Effect::CallProvider { .. }));
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `Timeout` 的工具支（`call_id: Some(_)`）：只在 `ToolsPending` 合法。
#[test]
fn timeout_tool_leg_legal_only_from_tools_pending() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::timeout_event(
            s.epoch(),
            Some(ToolCallId::new("call_1")),
        ));

        if matches!(status, TurnStatus::ToolsPending) {
            // 唯一的槽超时收敛：状态推进回 Thinking，跟 ToolFailed 同构。
            assert_eq!(s.status(), TurnStatus::Thinking);
            assert_eq!(effects.len(), 2, "TurnStatusChanged + CallProvider");
        } else {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        }
    }
}

/// `Cancel`：三个非终态统一合法；两个终态是 016 的裁决点——`ProtocolViolation`，
/// 且**不 bump epoch**（没有东西在飞可取消）。
#[test]
fn cancel_legal_from_non_terminal_states_violation_from_terminal() {
    for status in all_statuses() {
        let mut s = session_at(&status);
        let before = observe(&s);
        let effects = s.step(support::cancel_event());

        if status.is_terminal() {
            assert_untouched(&before, observe(&s), &status);
            assert_violation(&effects, &status);
        } else {
            assert_eq!(
                s.status(),
                TurnStatus::Failed(agent_core::Failure::Cancelled)
            );
            assert_eq!(s.epoch(), before.epoch.next(), "取消必须 bump epoch");
            assert!(
                effects
                    .iter()
                    .any(|e| matches!(e, Effect::CancelInFlight { .. })),
                "必须发 CancelInFlight"
            );
        }
    }
}
