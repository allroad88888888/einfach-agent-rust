//! 026 独立测试：屏障链路（020 的 barrier 谓词接上真日志）。
//!
//! `mark_irreversible(call_id)` 之后，落地那次结果的 entry 带 `barrier: true`；
//! `undo_turn` 走到它要停下（`Blocked`，`barrier_seq` 指向正确的那条 entry）；
//! `undo_turn_force` 恰放行一条屏障——同一轮里还有第二条屏障时，force 一次只
//! 越过遇到的第一条，仍然会在第二条前面再次 `Blocked`（026 实做记录判断 7）。

mod support;

use agent_core::{ToolCallId, UndoReport};
use support::session::thinking_session;
use support::{provider_done_tool_use, tool_result_event};

#[test]
fn a_barrier_entry_blocks_undo_turn_at_the_right_seq() {
    let mut session = thinking_session();
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")]));

    session.mark_irreversible(ToolCallId::new("call_1"));
    let _ = session.step(tool_result_event(epoch, "call_1", "r1"));
    let barrier_entry_seq = session.history().last().unwrap().seq;
    assert!(session.history().last().unwrap().meta.barrier, "call_1 落地的这条 entry 该带 barrier");

    let _ = session.step(tool_result_event(epoch, "call_2", "r2"));
    assert!(!session.history().last().unwrap().meta.barrier, "call_2 没被标记，这条不是屏障");

    let report = session.undo_turn();
    match report {
        UndoReport::Blocked { entries, barrier_seq } => {
            assert_eq!(entries, 1, "只有 call_2 落地那条比屏障新，该被先弹掉");
            assert_eq!(barrier_seq, barrier_entry_seq, "barrier_seq 必须指向 call_1 落地的那条 entry");
        }
        other => panic!("期望 Blocked，得到 {other:?}"),
    }

    // 屏障没被越过：call_1 的结果仍然在状态里（还没转出 ToolsPending）。
    assert_eq!(session.status(), agent_core::TurnStatus::ToolsPending);
}

#[test]
fn undo_turn_force_crosses_exactly_one_barrier_then_blocks_on_the_next() {
    let mut session = thinking_session();
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")]));

    session.mark_irreversible(ToolCallId::new("call_1"));
    session.mark_irreversible(ToolCallId::new("call_2"));
    let _ = session.step(tool_result_event(epoch, "call_1", "r1"));
    let first_barrier_seq = session.history().last().unwrap().seq;
    let _ = session.step(tool_result_event(epoch, "call_2", "r2"));
    let second_barrier_seq = session.history().last().unwrap().seq;
    assert_ne!(first_barrier_seq, second_barrier_seq);

    // 游标正下方就是屏障：一步都还没弹，立刻 Blocked。
    let report = session.undo_turn();
    match report {
        UndoReport::Blocked { entries, barrier_seq } => {
            assert_eq!(entries, 0);
            assert_eq!(barrier_seq, second_barrier_seq);
        }
        other => panic!("期望 Blocked，得到 {other:?}"),
    }

    // force：越过第二条屏障，紧接着撞上第一条，仍然 Blocked——不是全放行。
    let report = session.undo_turn_force();
    match report {
        UndoReport::Blocked { entries, barrier_seq } => {
            assert_eq!(entries, 1, "只放行了刚才那一条屏障");
            assert_eq!(barrier_seq, first_barrier_seq, "第二条屏障仍然拦着");
        }
        other => panic!("期望 Blocked，得到 {other:?}"),
    }

    // 再来一次 force：越过第一条屏障，这一轮再没有屏障了，一路退到底。
    let report = session.undo_turn_force();
    assert!(matches!(report, UndoReport::Applied { .. }), "没有更多屏障，该一路退完");
    assert_eq!(session.status(), agent_core::TurnStatus::Idle);
}

#[test]
fn marking_the_same_call_id_irreversible_twice_is_idempotent() {
    let mut session = thinking_session();
    let epoch = session.epoch();
    let _ = session.step(provider_done_tool_use(epoch, &[("call_1", "srv:fs/read")]));

    session.mark_irreversible(ToolCallId::new("call_1"));
    session.mark_irreversible(ToolCallId::new("call_1"));
    let _ = session.step(tool_result_event(epoch, "call_1", "r1"));

    // 收敛之后状态已经离开 ToolsPending，barrier 落在「call_1 落地」这一条，而不是
    // 后续被追加的收敛动作——这里只需确认重复登记没有产生第二条屏障、也没有 panic。
    let barrier_entries: Vec<_> = session.history().entries().filter(|e| e.meta.barrier).collect();
    assert_eq!(barrier_entries.len(), 1, "重复登记同一个 call_id 只应该产生一条屏障");
}
