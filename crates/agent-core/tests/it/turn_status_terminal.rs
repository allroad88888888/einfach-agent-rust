//! 验收 8：`TurnStatus::is_terminal()` 穷举——`Done{..}`/`Failed(..)` 为真，
//! 其余（`Idle`/`Thinking`/`ToolsPending`）为假。
//!
//! 这也是「答完了」的唯一出口：runner 靠这个判定收工，效果列表是否为空是歧义的
//! （见 001 实做记录判断 5）。

use agent_core::{ErrorClass, Failure, TurnStatus};

#[test]
fn terminal_statuses() {
    assert!(TurnStatus::Done { truncated: false }.is_terminal());
    assert!(TurnStatus::Done { truncated: true }.is_terminal());
    assert!(TurnStatus::Failed(Failure::Cancelled).is_terminal());
    assert!(TurnStatus::Failed(Failure::Provider(ErrorClass::Auth)).is_terminal());
}

#[test]
fn non_terminal_statuses() {
    assert!(!TurnStatus::Idle.is_terminal());
    assert!(!TurnStatus::Thinking.is_terminal());
    assert!(!TurnStatus::ToolsPending.is_terminal());
}
