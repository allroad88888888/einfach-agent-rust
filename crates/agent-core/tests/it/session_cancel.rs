//! 026 等价重写自 `cancel_any_state.rs`：`Event::Cancel` 的内部子分支（016）
//! ——epoch 怎么 bump、`CancelInFlight` 带的是不是旧 epoch、`ToolsPending` 的槽是不是
//! 真的全弃、终态那两格具体落成什么。
//!
//! 一处形状变化：M1 靠 `st.epoch = Epoch(5)` 直接摆一个非零世代，`Session` 没有那条
//! 后门——这里用 `undo_turn`（另一个 bump 世代的动作，红线 6）把世代推起来，顺带
//! 证明了两条 bump 路径产出的是同一个东西。

use crate::support;
use agent_core::{Effect, Epoch, Failure, Notice, TurnStatus};

use crate::support::session::{
    new_session, observe, session_at, session_with_pending_tools, thinking_session,
};

/// `Idle` + `Cancel`：`Idle` 不是终态，跟 `Thinking`/`ToolsPending` 走同一条路径，
/// 不特殊对待成 no-op。
#[test]
fn cancel_from_idle_bumps_epoch_and_fails_as_cancelled() {
    let mut s = new_session();
    let old_epoch = s.epoch();

    let effects = s.step(support::cancel_event());

    assert_eq!(s.status(), TurnStatus::Failed(Failure::Cancelled));
    assert_eq!(s.epoch(), old_epoch.next());
    assert_eq!(
        effects,
        vec![
            Effect::CancelInFlight { epoch: old_epoch },
            Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::Failed(Failure::Cancelled)
            }),
        ]
    );
}

/// `Thinking` + `Cancel`：provider 调用在飞时取消——`CancelInFlight` 带的必须是
/// **取消前**（在飞请求所属）的那个 epoch，不是 bump 之后的新 epoch，否则宿主没法
/// 把它跟真正在飞的那次请求对上号。
#[test]
fn cancel_from_thinking_carries_the_pre_bump_epoch() {
    let mut s = new_session();
    // 先把世代推到非零，好让「旧 epoch」和「新 epoch」在断言里长得不一样。
    let _ = s.step(support::user_input_event("hi"));
    let _ = s.undo_turn();
    assert_eq!(s.epoch(), Epoch(1), "undo 也 bump 世代（红线 6）");
    let _ = s.step(support::user_input_event("hi"));
    assert_eq!(s.status(), TurnStatus::Thinking);

    let effects = s.step(support::cancel_event());

    assert_eq!(s.epoch(), Epoch(2));
    assert_eq!(
        effects[0],
        Effect::CancelInFlight { epoch: Epoch(1) },
        "必须是旧 epoch，不是 bump 后的新 epoch"
    );
}

/// `ToolsPending` + `Cancel`：验收原文点名的例子——「槽全弃」。不清的话终态里留着
/// 一堆再也不会被回执认领的 `Pending` 槽，是自身即会误导的死数据。
#[test]
fn cancel_from_tools_pending_discards_all_slots() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);

    let effects = s.step(support::cancel_event());

    assert_eq!(s.status(), TurnStatus::Failed(Failure::Cancelled));
    assert!(s.tool_slots().is_empty(), "槽全弃");
    assert!(s.tools_converged(), "槽全弃之后没有东西要等了");
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { .. }))
    );
}

/// 终态收到 `Cancel`：`ProtocolViolation`，状态（含 `epoch`）逐字段不变——没有东西
/// 可取消，不 bump、不发 `CancelInFlight`，日志里也不多一条。
#[test]
fn cancel_from_terminal_states_is_a_protocol_violation_and_does_not_bump_epoch() {
    for status in [
        TurnStatus::Done { truncated: false },
        TurnStatus::Failed(Failure::Cancelled),
    ] {
        let mut s = session_at(&status);
        let before = observe(&s);

        let effects = s.step(support::cancel_event());

        assert_eq!(
            observe(&s),
            before,
            "{status:?}：终态收到 Cancel 不该改任何东西"
        );
        assert!(
            matches!(
                effects.as_slice(),
                [Effect::Emit(Notice::ProtocolViolation { .. })]
            ),
            "{status:?}"
        );
        assert!(
            !effects
                .iter()
                .any(|e| matches!(e, Effect::CancelInFlight { .. })),
            "{status:?}：终态不该发 CancelInFlight"
        );
    }
}

/// 连续两次取消（用户手滑按了两下 Ctrl-C）：第二次落在已经终结的状态上，必须是
/// 显式违规而不是 panic 或者诡异地再 bump 一次 epoch。
#[test]
fn double_cancel_second_one_hits_the_terminal_case() {
    let mut s = thinking_session();

    let _ = s.step(support::cancel_event());
    assert_eq!(s.status(), TurnStatus::Failed(Failure::Cancelled));
    let epoch_after_first = s.epoch();

    let effects = s.step(support::cancel_event());

    assert_eq!(s.epoch(), epoch_after_first, "第二次取消不该再 bump");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));
}
