//! 026 独立测试：`begin_turn`——turn_id 递增、状态回 Idle、工具槽清空、消息历史
//! 保留（以及文档点名的另外两件事：本轮预算清零、上限延续）。

mod support;

use agent_core::TurnStatus;
use support::session::new_session;
use support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};

#[test]
fn begin_turn_advances_the_turn_resets_the_slate_and_keeps_the_conversation() {
    let mut session = new_session();
    session.set_max_turns(7);
    session.set_max_retries(9);

    let _ = session.step(user_input_event("turn one"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read")]));
    let _ = session.step(tool_result_event(epoch, "call_1", "r1"));
    let _ = session.step(provider_done_end_turn(epoch, "done with turn one"));

    let turn_id_before = session.turn_id();
    let messages_before = session.messages();
    assert_eq!(session.status(), TurnStatus::Done { truncated: false });
    assert!(session.turns_used() > 0);

    session.begin_turn();

    assert_eq!(session.turn_id(), turn_id_before + 1, "turn_id 递增");
    assert_eq!(session.status(), TurnStatus::Idle, "状态回 Idle");
    assert!(session.tool_slots().is_empty(), "工具槽清空");
    assert_eq!(session.messages(), messages_before, "消息历史保留");

    assert_eq!(session.turns_used(), 0, "本轮已用的轮数清零");
    assert_eq!(session.retries_used(), 0, "本轮已用的重试次数清零");
    assert_eq!(session.max_turns(), 7, "上限延续，不回到默认值");
    assert_eq!(session.max_retries(), 9, "上限延续，不回到默认值");
}

#[test]
fn begin_turn_writes_its_own_entry_carrying_the_new_turn_id() {
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "bye"));
    let history_len_before = session.history_len();

    session.begin_turn();

    assert_eq!(
        session.history_len(),
        history_len_before + 1,
        "begin_turn 本身也是一次写入,留痕"
    );
    let last = session.history().last().expect("刚写完不该是空的");
    assert_eq!(
        last.meta.turn_id,
        session.turn_id(),
        "这条 entry 记的是新轮的号，undo_turn 才能把它跟新轮的其余 entry 分在一组"
    );
}

#[test]
fn a_terminal_status_receiving_user_input_directly_is_still_a_protocol_violation() {
    // begin_turn 是显式命令：不调它、直接往 Done 状态喂 UserInput 仍然是违规，
    // 不会被当成「隐式开新轮」。
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "bye"));
    assert_eq!(session.status(), TurnStatus::Done { truncated: false });

    let history_len_before = session.history_len();
    let effects = session.step(user_input_event("no begin_turn"));

    assert_eq!(
        session.status(),
        TurnStatus::Done { truncated: false },
        "违规转移不改状态"
    );
    assert_eq!(session.history_len(), history_len_before, "违规转移不留痕");
    assert!(
        effects.iter().any(|e| matches!(
            e,
            agent_core::Effect::Emit(agent_core::Notice::ProtocolViolation { .. })
        )),
        "该有一条可观测的 ProtocolViolation 通报，而不是静默什么都不做"
    );
}
