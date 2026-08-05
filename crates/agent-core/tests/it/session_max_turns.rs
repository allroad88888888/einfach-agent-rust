//! 026 等价重写自 `max_turns.rs`：撞顶时能停住，且调用方能区分「答完了」
//! （`Done{truncated:false}`）和「被截断了」（`Done{truncated:true}`）。

mod support;

use agent_core::{Effect, Notice, TurnStatus};

use support::session::{new_session, thinking_session};

/// 默认上限是 32——016 的裁决，钉成回归断言。原子图版本多一层意思：这个默认值
/// 来自 `graph::Slot::default_value()`，而它取的是 `engine::state` 的同一个常量，
/// 两条路的预算不可能分家。
#[test]
fn default_max_turns_is_32() {
    assert_eq!(new_session().max_turns(), 32);
}

/// 撞顶时不再发 `CallProvider`：`max_turns=1`，第一次 `UserInput` 合法用掉唯一的
/// 预算；工具跑完收敛之后本该接着调 provider，但预算已经耗尽，应该改落
/// `Done{truncated:true}`。
#[test]
fn hitting_the_cap_after_tool_convergence_lands_done_truncated_instead_of_calling_again() {
    let mut s = new_session();
    s.set_max_turns(1);

    let _ = s.step(support::user_input_event("读一下 a.txt"));
    assert_eq!(s.turns_used(), 1);

    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    assert_eq!(s.status(), TurnStatus::ToolsPending);

    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "内容"));

    assert_eq!(s.status(), TurnStatus::Done { truncated: true }, "撞顶，被截断");
    assert!(
        !effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })),
        "撞顶之后不该再发 CallProvider：{effects:?}"
    );
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: true }
        })]
    );
}

/// 「答完了」和「被截断了」必须能区分：同样是 `Done`，`truncated` 字段不同。
#[test]
fn answered_and_truncated_are_distinguishable() {
    let mut answered = thinking_session();
    let _ = answered.step(support::provider_done_end_turn(answered.epoch(), "done"));
    assert_eq!(answered.status(), TurnStatus::Done { truncated: false });

    let mut truncated = new_session();
    truncated.set_max_turns(0);
    let effects = truncated.step(support::user_input_event("hi"));
    assert_eq!(truncated.status(), TurnStatus::Done { truncated: true });
    assert_ne!(answered.status(), truncated.status(), "两种结束方式必须能区分");
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: true }
        })]
    );
}

/// `max_turns=0`（古怪但合法的宿主配置）：第一次 `UserInput` 就该直接落
/// `Done{truncated:true}`——用户消息仍然无条件进历史。
#[test]
fn zero_max_turns_rejects_the_very_first_attempt() {
    let mut s = new_session();
    s.set_max_turns(0);

    let effects = s.step(support::user_input_event("hi"));

    assert_eq!(s.status(), TurnStatus::Done { truncated: true });
    assert_eq!(s.turns_used(), 0, "从没真的发过 CallProvider，不该计数");
    assert_eq!(s.messages().len(), 1, "用户消息仍然进历史");
    assert!(!effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })));
}

/// `turns_used` 精确地随每一次 `CallProvider` 递增。
#[test]
fn turns_used_increments_once_per_call_provider() {
    let mut s = new_session();

    let _ = s.step(support::user_input_event("a"));
    assert_eq!(s.turns_used(), 1);

    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok"));
    assert_eq!(s.turns_used(), 2, "工具收敛之后重新调 provider 也算一轮");

    let _ = s.step(support::provider_done_end_turn(s.epoch(), "done"));
    assert_eq!(s.turns_used(), 2, "EndTurn 不发 CallProvider，不该再计数");
}

/// `begin_turn` 把本轮预算清零、状态回 `Idle`，消息历史与两个上限延续——
/// 这是 M1 宿主 `agent_cli::next_turn` 那份重置在 command 层的落点。
#[test]
fn begin_turn_resets_the_per_turn_budget_and_keeps_the_conversation() {
    let mut s = new_session();
    s.set_max_turns(5);

    let _ = s.step(support::user_input_event("第一轮"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "答完了"));
    assert_eq!(s.turns_used(), 1);
    let turn_before = s.turn_id();

    s.begin_turn();

    assert_eq!(s.status(), TurnStatus::Idle);
    assert_eq!(s.turns_used(), 0);
    assert_eq!(s.retries_used(), 0);
    assert_eq!(s.max_turns(), 5, "上限延续，不回默认值");
    assert_eq!(s.messages().len(), 2, "消息历史延续");
    assert_eq!(s.next_message_id().0, 3, "消息号计数器延续");
    assert_eq!(s.turn_id(), turn_before + 1);

    // 新一轮能正常开：`Idle + UserInput` 那一格。
    let effects = s.step(support::user_input_event("第二轮"));
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert!(effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })));
}
