//! 026 独立测试：`undo_step` 的 batch 粒度——一轮多条 entry，`undo_step` 恰退
//! 一条；「回滚式」锚点：`undo_step` 之后 `tools_converged()` 立即反映（不经过
//! 任何 `step()` 调用），这是 003 验收 3「收敛判断不是计数器、是扫描」在
//! `Session` 一侧唯一够得着的复现方式——原子图版本没有直接给字段赋值的后门。

mod support;

use agent_core::UndoReport;
use support::session::{new_session, session_with_pending_tools};
use support::{provider_done_end_turn, tool_result_event, user_input_event};

#[test]
fn undo_step_flips_convergence_back_immediately_without_a_step_call() {
    let mut session = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    assert!(!session.tools_converged(), "唯一的槽还是 Pending");

    let epoch = session.epoch();
    let _ = session.step(tool_result_event(epoch, "call_1", "ok"));
    assert!(session.tools_converged(), "唯一的槽落地，槽位清空，收敛");
    assert_eq!(session.status(), agent_core::TurnStatus::Thinking);

    let report = session.undo_step();
    assert!(matches!(report, UndoReport::Applied { entries: 1, .. }));

    // 没有调用任何 step()：derived 是现查出来的，不是维护出来的缓存。
    assert!(!session.tools_converged(), "回滚之后立刻应该看到 Pending 那个槽回来了");
    assert_eq!(session.status(), agent_core::TurnStatus::ToolsPending);
}

#[test]
fn undo_step_reverts_exactly_one_entry_leaving_earlier_entries_intact() {
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "bye"));

    let cursor_before = session.cursor();
    let history_len_before = session.history_len();
    let first_message_before = session.messages().get(0).cloned();

    let report = session.undo_step();
    assert!(matches!(report, UndoReport::Applied { entries: 1, .. }));

    assert_eq!(session.cursor(), cursor_before - 1, "游标恰好退一步");
    assert_eq!(session.history_len(), history_len_before, "undo 不物理删条目，日志长度不变");
    assert_eq!(session.status(), agent_core::TurnStatus::Thinking, "只退了 ProviderDone 那一条，不是整轮");
    assert_eq!(session.messages().len(), 1, "ProviderDone 追加的助手消息被退掉了");
    assert_eq!(session.messages().get(0).cloned(), first_message_before, "更早的 entry（用户消息）原封不动");
}

#[test]
fn redo_step_is_the_inverse_of_undo_step() {
    let mut session = new_session();
    let _ = session.step(user_input_event("hi"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "bye"));
    let after_both = session.primitives();

    let _ = session.undo_step();
    let after_first_undo = session.primitives();
    assert_ne!(after_first_undo, after_both);

    let report = session.redo_step();
    assert!(matches!(report, UndoReport::Applied { entries: 1, .. }));
    assert_eq!(session.primitives(), after_both);
}
