//! 026 / M2 验收的核心句：**undo 一整 turn 之后所有 primitive 逐值回退、所有 derived
//! 重算一致**，redo 反演；`barrier=true` 的 entry 让 `undo_turn` 返回 `Blocked`，
//! `undo_turn_force` 才越过（027 的 `/undo!` 后端）。
//!
//! M1 没有对应文件——这是 026 长出来的能力本身。

mod support;

use agent_core::{AgentValue, AtomKey, Slot, ToolCallId, TurnStatus, UndoReport};

use support::session::{new_session, session_with_pending_tools};

/// 跑完一整轮并 `begin_turn`，返回「第一轮刚结束时」的完整快照。
fn one_finished_turn() -> (agent_core::Session, Vec<(AtomKey, AgentValue)>) {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("第一轮"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "答完了"));
    let snapshot = s.primitives();
    (s, snapshot)
}

/// **验收核心句**：第二轮跑完，`undo_turn` 一次退回第一轮结束的那一刻——所有
/// primitive 逐值相等，derived 跟着重算（不是停在旧值），日志游标退到 turn 边界。
#[test]
fn undoing_a_whole_turn_restores_every_primitive_and_recomputes_every_derived() {
    let (mut s, after_turn_one) = one_finished_turn();
    s.begin_turn();
    let _ = s.step(support::user_input_event("第二轮"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    assert!(!s.tools_converged(), "第二轮有工具在飞");
    assert_ne!(s.primitives(), after_turn_one);

    let recomputes = s.debug_recompute_count();
    let report = s.undo_turn();

    assert_eq!(
        report,
        UndoReport::Applied { entries: 3, turn_id: 2 },
        "begin_turn + user_input + provider_done 三条都属于 turn 2"
    );
    assert_eq!(s.primitives(), after_turn_one, "所有 primitive 逐值回退");
    assert!(
        s.debug_recompute_count() > recomputes,
        "derived 必须重算，不是停在旧值"
    );
    assert!(s.tools_converged(), "回到第一轮结束时：没有工具在飞");
    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    assert_eq!(s.cursor(), 2, "游标退到 turn 边界");
    assert_eq!(s.history_len(), 5, "条目没被物理弹掉——否则 redo 无从谈起");
}

/// `redo_turn` 恰好反演 `undo_turn`：同一批条目、同一个 turn，值全部回到原样。
#[test]
fn redo_turn_is_the_exact_inverse_of_undo_turn() {
    let (mut s, _) = one_finished_turn();
    s.begin_turn();
    let _ = s.step(support::user_input_event("第二轮"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let before_undo = s.primitives();
    let cursor_before = s.cursor();

    assert!(matches!(s.undo_turn(), UndoReport::Applied { .. }));
    let report = s.redo_turn();

    assert_eq!(report, UndoReport::Applied { entries: 3, turn_id: 2 });
    assert_eq!(s.primitives(), before_undo, "redo 之后逐值回到原样");
    assert_eq!(s.cursor(), cursor_before);
    assert!(!s.tools_converged(), "derived 也跟着回来了");
}

/// 连续 undo 跨越多个 turn 边界，每次都停在正确的位置；退到底之后是 `Nothing`，
/// 不是 panic。
#[test]
fn consecutive_undos_walk_turn_by_turn_and_then_report_nothing() {
    let mut s = new_session();
    let fresh = s.primitives();
    for round in 1..=3 {
        if round > 1 {
            s.begin_turn();
        }
        let _ = s.step(support::user_input_event("说话"));
        let _ = s.step(support::provider_done_end_turn(s.epoch(), "答话"));
    }

    for expected_turn in [3u64, 2, 1] {
        match s.undo_turn() {
            UndoReport::Applied { turn_id, .. } => assert_eq!(turn_id, expected_turn),
            other => panic!("期待 Applied，收到 {other:?}"),
        }
    }
    assert_eq!(s.primitives(), fresh, "退到底就是开局那份状态");
    assert_eq!(s.undo_turn(), UndoReport::Nothing, "到头了要明确报，不 panic");
}

/// 020 的屏障接上真日志：宿主标记过 `Irreversible` 的那次调用，它的结果落地那一条
/// entry 带 `barrier: true`，`undo_turn` 走到它**停下**（`Blocked`），游标停在屏障
/// 后一格——屏障本身没被越过。
#[test]
fn a_barrier_entry_blocks_undo_instead_of_silently_rolling_it_back() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:shell/exec")]);
    s.mark_irreversible(ToolCallId::new("call_1"));

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "rm 干完了"));
    let barrier_entry = s.last_entry().unwrap();
    assert!(barrier_entry.meta.barrier, "不可逆调用的结果那一条是屏障");
    let barrier_seq = barrier_entry.seq;

    let _ = s.step(support::provider_done_end_turn(s.epoch(), "干完了"));
    let cursor_before = s.cursor();

    let report = s.undo_turn();

    assert_eq!(
        report,
        UndoReport::Blocked { entries: 1, barrier_seq },
        "屏障之上的那一条已经退了，屏障本身停在门口"
    );
    assert_eq!(s.cursor(), cursor_before - 1);
    assert_eq!(
        s.status(),
        TurnStatus::Thinking,
        "只退了屏障之上那一条（收尾的 provider_done），工具结果那一条还在"
    );

    // 幂等：再问一次还是同样的答案，History 不记「已经问过了」。
    assert_eq!(s.undo_turn(), UndoReport::Blocked { entries: 0, barrier_seq });
    assert_eq!(s.cursor(), cursor_before - 1, "游标一动不动");
}

/// `undo_turn_force`（`/undo!`）越过**第一条**屏障继续退——一次确认只放行一条
/// 不可逆操作。
#[test]
fn undo_turn_force_crosses_exactly_one_barrier() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:shell/exec"), ("call_2", "srv:shell/exec")]);
    s.mark_irreversible(ToolCallId::new("call_1"));
    s.mark_irreversible(ToolCallId::new("call_2"));

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "第一次 rm"));
    let first_barrier = s.last_entry().unwrap().seq;
    let _ = s.step(support::tool_result_event(s.epoch(), "call_2", "第二次 rm"));
    let second_barrier = s.last_entry().unwrap().seq;
    assert_ne!(first_barrier, second_barrier);

    // 普通 undo：撞上最近那条屏障，一条都退不动。
    assert_eq!(
        s.undo_turn(),
        UndoReport::Blocked { entries: 0, barrier_seq: second_barrier }
    );

    // 强制：越过第二条，接着走到第一条又停下。
    assert_eq!(
        s.undo_turn_force(),
        UndoReport::Blocked { entries: 1, barrier_seq: first_barrier },
        "一次确认只放行一条"
    );

    // 再强制一次：越过第一条，这一轮剩下的条目一路退完。
    let report = s.undo_turn_force();
    assert!(matches!(report, UndoReport::Applied { turn_id: 1, .. }), "{report:?}");
    assert_eq!(s.status(), TurnStatus::Idle);
}

/// undo 之后再写新内容 → **默认覆盖 redo 尾**（分支是显式操作，不是默认行为），
/// 并报一条 `DropEvent::RedoTail` 供宿主转发给 `SessionStore::drop_after`（011）。
#[test]
fn writing_after_an_undo_discards_the_redo_tail_and_reports_it() {
    let (mut s, _) = one_finished_turn();
    s.begin_turn();
    let _ = s.step(support::user_input_event("第二轮"));
    let _ = s.take_drop_events();

    let _ = s.undo_turn();
    assert!(s.take_drop_events().is_empty(), "undo 本身不产生裁剪事件");

    // 游标不在栈顶时写新内容。
    s.begin_turn();
    let _ = s.step(support::user_input_event("改主意了"));

    let events = s.take_drop_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, agent_store::DropEvent::RedoTail { .. })),
        "{events:?}"
    );
    assert_eq!(s.redo_turn(), UndoReport::Nothing, "被丢的分支回不去了");
}

/// undo 走的是 019 的 applier，键是逻辑键：回滚写回去的是 `Change.prev`，
/// 一个都不少。这里直接对着日志核对一条 entry 的逆操作。
#[test]
fn undo_writes_back_exactly_the_prev_of_every_change() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("hi"));

    let entry = s.last_entry().unwrap().clone();
    let _ = s.undo_step();

    let now: std::collections::BTreeMap<AtomKey, AgentValue> = s.primitives().into_iter().collect();
    for change in &entry.changes {
        assert_eq!(
            now.get(&change.key),
            Some(&change.prev),
            "{:?} 应该被写回 prev",
            change.key
        );
    }
    assert_eq!(
        now.get(&AtomKey::Agent(support::agent(), Slot::Status)),
        Some(&AgentValue::Status(TurnStatus::Idle))
    );
}
