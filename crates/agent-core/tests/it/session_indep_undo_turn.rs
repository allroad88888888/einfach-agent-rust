//! 026 独立测试：`undo_turn` 全回退是 M2 验收的核心句——两轮对话后 undo_turn，
//! 所有 primitive 逐值回退到第一轮结束时的快照，`tools_converged()` 等读口一致；
//! `redo_turn` 反演回第二轮结束时的状态。

use agent_core::UndoReport;
use crate::support::session::new_session;
use crate::support::{
    provider_done_end_turn, provider_done_tool_use, tool_result_event, user_input_event,
};

#[test]
fn undoing_a_whole_turn_restores_every_primitive_and_recomputes_every_derived() {
    let mut session = new_session();

    // 第一轮：一次工具调用往返，正常结束。
    let _ = session.step(user_input_event("turn one"));
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read")]));
    let _ = session.step(tool_result_event(epoch, "call_1", "result one"));
    let _ = session.step(provider_done_end_turn(epoch, "answer one"));

    let snapshot_after_turn1 = session.primitives();
    let tools_converged_after_turn1 = session.tools_converged();
    let messages_after_turn1 = session.messages();
    assert_eq!(session.turn_id(), 1);

    // 第二轮：显式开新轮，内容跟第一轮不一样，确保状态真的动了。
    session.begin_turn();
    let _ = session.step(user_input_event("turn two"));
    let epoch2 = session.epoch();
    let _ = session.step(provider_done_tool_use(
        epoch2,
        &[("call_2", "srv:fs/read"), ("call_3", "srv:fs/read")],
    ));
    let _ = session.step(tool_result_event(epoch2, "call_2", "result two"));
    let _ = session.step(tool_result_event(epoch2, "call_3", "result three"));
    let _ = session.step(provider_done_end_turn(epoch2, "answer two"));

    let snapshot_after_turn2 = session.primitives();
    assert_ne!(
        snapshot_after_turn1, snapshot_after_turn2,
        "两轮内容不同，状态不该相等"
    );
    assert_eq!(session.turn_id(), 2);

    let report = session.undo_turn();
    match report {
        UndoReport::Applied { turn_id, .. } => assert_eq!(turn_id, 2, "回退的应该是第二轮"),
        other => panic!("期望 Applied，得到 {other:?}"),
    }

    assert_eq!(
        session.primitives(),
        snapshot_after_turn1,
        "undo 一整 turn 后所有 primitive 逐值回退"
    );
    assert_eq!(
        session.tools_converged(),
        tools_converged_after_turn1,
        "derived 重算后与第一轮结束时一致"
    );
    assert_eq!(session.messages(), messages_after_turn1);
    assert_eq!(
        session.status(),
        agent_core::TurnStatus::Done { truncated: false }
    );

    // redo：反演回第二轮结束时的状态。
    let redo = session.redo_turn();
    match redo {
        UndoReport::Applied { turn_id, .. } => assert_eq!(turn_id, 2),
        other => panic!("期望 Applied，得到 {other:?}"),
    }
    assert_eq!(
        session.primitives(),
        snapshot_after_turn2,
        "redo_turn 是 undo_turn 的精确反演"
    );
}

#[test]
fn undo_turn_then_redo_turn_round_trip_is_a_no_op_on_reads() {
    let mut session = new_session();
    let _ = session.step(user_input_event("hello"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "world"));

    let before = session.primitives();
    let cursor_before = session.cursor();

    let undo = session.undo_turn();
    assert!(matches!(undo, UndoReport::Applied { .. }));
    assert_ne!(
        session.primitives(),
        before,
        "第一轮本身也该被回退（回到会话开局）"
    );

    let redo = session.redo_turn();
    assert!(matches!(redo, UndoReport::Applied { .. }));
    assert_eq!(session.primitives(), before);
    assert_eq!(session.cursor(), cursor_before);
}
