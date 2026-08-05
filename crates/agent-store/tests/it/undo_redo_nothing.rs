//! 验收 6（017）：`Nothing` 的三种触发——空日志、顶端 redo、底端 undo。
//! 钉死接口（重建版本）见 `undo_redo_roundtrip.rs` 顶部注释。

use crate::common::*;

use agent_store::{History, Store, UndoOutcome, record_set};

#[derive(Clone, Copy, Debug, PartialEq)]
struct M {
    turn: u32,
}

fn same_turn(a: &M, b: &M) -> bool {
    a.turn == b.turn
}

fn no_barrier(_: &M) -> bool {
    false
}

fn assert_nothing(outcome: UndoOutcome<String, TestValue, M>) {
    match outcome {
        UndoOutcome::Nothing => {}
        UndoOutcome::Applied(_) => panic!("expected Nothing, got Applied"),
        UndoOutcome::Blocked { .. } => panic!("expected Nothing, got Blocked"),
    }
}

#[test]
fn empty_log_undo_and_redo_are_both_nothing() {
    let mut history: History<String, TestValue, M> = History::new();
    assert_eq!(history.cursor(), 0);
    assert!(!history.can_undo());
    assert!(!history.can_redo());

    assert_nothing(history.undo_one(no_barrier));
    assert_nothing(history.undo_turn(same_turn, no_barrier));
    assert_nothing(history.redo_one());
    assert_nothing(history.redo_turn(same_turn));
}

#[test]
fn redo_at_the_top_is_nothing() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    let c = record_set(&store, "p".to_string(), p, num(1.0)).unwrap();
    history.append(M { turn: 1 }, vec![c]);

    assert!(!history.can_redo());
    assert_nothing(history.redo_one());
    assert_nothing(history.redo_turn(same_turn));
}

#[test]
fn undo_at_the_bottom_is_nothing() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    let c = record_set(&store, "p".to_string(), p, num(1.0)).unwrap();
    history.append(M { turn: 1 }, vec![c]);

    match history.undo_one(no_barrier) {
        UndoOutcome::Applied(entries) => {
            for e in &entries {
                for c in e.changes.iter().rev() {
                    store.set(p, c.prev.clone());
                }
            }
        }
        _ => panic!("expected Applied"),
    }
    assert!(!history.can_undo());
    assert_eq!(history.cursor(), 0);

    assert_nothing(history.undo_one(no_barrier));
    assert_nothing(history.undo_turn(same_turn, no_barrier));
}
