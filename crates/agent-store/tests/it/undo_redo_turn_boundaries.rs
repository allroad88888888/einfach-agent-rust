//! 验收 2（017）+「跨 turn 边界停位正确」：三个 turn，各写两条 entry，连续三次
//! undo_turn，每次都要停在正确的 turn 边界上——逐次断言 cursor 与 store 值。
//! 钉死接口（重建版本）见 `undo_redo_roundtrip.rs` 顶部注释。

mod common;
use common::*;

use agent_store::{AtomId, History, Store, UndoOutcome, record_set};

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

/// 调用方闭包：undo 方向——entries 已倒序，条目内 changes 也倒序，写 prev。
/// 返回本次实际弹出的 entry 数，供断言用。
fn apply_undo(
    store: &Store<TestValue>,
    atom: AtomId,
    outcome: UndoOutcome<String, TestValue, M>,
) -> usize {
    let entries = match outcome {
        UndoOutcome::Applied(e) => e,
        UndoOutcome::Blocked { .. } => panic!("expected Applied, got Blocked"),
        UndoOutcome::Nothing => panic!("expected Applied, got Nothing"),
    };
    let count = entries.len();
    for entry in &entries {
        for c in entry.changes.iter().rev() {
            store.set(atom, c.prev.clone());
        }
    }
    count
}

#[test]
fn three_consecutive_undo_turn_stop_at_each_turn_boundary() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    // 三个 turn，各两条 entry：0->1->2 (turn1), 2->3->4 (turn2), 4->5->6 (turn3)。
    for (turn, nexts) in [(1u32, [1.0, 2.0]), (2u32, [3.0, 4.0]), (3u32, [5.0, 6.0])] {
        for next in nexts {
            let c = record_set(&store, "p".to_string(), p, num(next)).unwrap();
            history.append(M { turn }, vec![c]);
        }
    }

    assert_eq!(store.get(p).as_number(), Some(6.0));
    assert_eq!(history.cursor(), 6);

    // 第一次 undo_turn：退回 turn3 两条，落在 turn2 末尾（p=4）。
    let n = apply_undo(&store, p, history.undo_turn(same_turn, no_barrier));
    assert_eq!(n, 2);
    assert_eq!(store.get(p).as_number(), Some(4.0));
    assert_eq!(history.cursor(), 4);

    // 第二次：退回 turn2 两条，落在 turn1 末尾（p=2）。
    let n = apply_undo(&store, p, history.undo_turn(same_turn, no_barrier));
    assert_eq!(n, 2);
    assert_eq!(store.get(p).as_number(), Some(2.0));
    assert_eq!(history.cursor(), 2);

    // 第三次：退回 turn1 两条，落在日志底（p=0）。
    let n = apply_undo(&store, p, history.undo_turn(same_turn, no_barrier));
    assert_eq!(n, 2);
    assert_eq!(store.get(p).as_number(), Some(0.0));
    assert_eq!(history.cursor(), 0);
    assert!(!history.can_undo());
}
