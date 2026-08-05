//! 026 独立测试：红线 6 端到端——undo（或 Cancel 后 undo）之后旧 epoch 的
//! `ToolResult` 喂 `step` 必须被闸挡掉：返回空 effects，`primitives()` 逐值不变。

mod support;

use agent_core::UndoReport;
use support::session::session_with_pending_tools;
use support::{cancel_event, tool_result_event};

#[test]
fn a_tool_result_from_before_an_undo_is_dropped() {
    let mut session = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let old_epoch = session.epoch();

    let report = session.undo_step();
    assert!(matches!(report, UndoReport::Applied { .. }));
    assert_ne!(session.epoch(), old_epoch, "undo 必须 bump epoch（红线 6）");

    let state_after_undo = session.primitives();
    let effects = session.step(tool_result_event(old_epoch, "call_1", "ghost result"));

    assert!(effects.is_empty(), "旧 epoch 的回执必须被闸挡掉，不产出任何 effect");
    assert_eq!(session.primitives(), state_after_undo, "被挡掉的回执不能改动任何 primitive");
}

#[test]
fn a_tool_result_from_before_a_cancel_then_undo_is_also_dropped() {
    let mut session = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let old_epoch = session.epoch();

    // 用户取消（Cancel 本身就 bump epoch，且不带 epoch，不过闸）。
    let _ = session.step(cancel_event());
    let epoch_after_cancel = session.epoch();
    assert_ne!(epoch_after_cancel, old_epoch, "Cancel 自己就 bump 了一次");

    // 撤销这次取消：undo 会再 bump 一次。
    let report = session.undo_step();
    assert!(matches!(report, UndoReport::Applied { .. }));
    assert_ne!(session.epoch(), epoch_after_cancel, "undo 是独立于 Cancel 的又一次 bump");

    let state_after_undo = session.primitives();
    let effects = session.step(tool_result_event(old_epoch, "call_1", "ghost result"));

    assert!(effects.is_empty(), "两代之前的回执照样要被挡掉");
    assert_eq!(session.primitives(), state_after_undo);
}

#[test]
fn redo_does_not_bump_epoch_again() {
    let mut session = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let _ = session.undo_step();
    let epoch_after_undo = session.epoch();

    let report = session.redo_step();
    assert!(matches!(report, UndoReport::Applied { .. }));
    assert_eq!(session.epoch(), epoch_after_undo, "redo 只是把状态追回去，不该再 bump 一次");
}

#[test]
fn cancel_itself_does_not_go_through_the_epoch_gate() {
    // Event::Cancel 不带 epoch（它就是 bump epoch 的那一方），任意时刻都该被处理，
    // 不会因为「epoch 对不上」被静默丢弃。
    let mut session = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let before = session.epoch();
    let effects = session.step(cancel_event());
    assert!(!effects.is_empty(), "Cancel 永远生效，不过闸");
    assert_ne!(session.epoch(), before);
}
