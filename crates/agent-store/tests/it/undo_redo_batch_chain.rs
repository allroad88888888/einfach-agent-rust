//! 验收 5（017）：batch 内同一个 atom 写两次，undo 该条目时必须把 `changes`
//! 倒序应用，prev 链才能咬合、回到 batch 之前的值——顺序反了会错误停在中间值上。
//! 钉死接口（重建版本）见 `undo_redo_roundtrip.rs` 顶部注释。

use crate::common::*;

use einfach_store::{Change, History, Store, UndoOutcome, record_set};

#[derive(Clone, Copy, Debug, PartialEq)]
struct M;

fn no_barrier(_: &M) -> bool {
    false
}

#[test]
fn undoing_a_batch_that_writes_the_same_atom_twice_applies_changes_in_reverse() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(1.0));
    let mut history: History<String, TestValue, M> = History::new();

    // 一次 batch 里对同一个 atom 连写两次：1 -> 2 -> 3。
    let mut changes: Vec<Change<String, TestValue>> = Vec::new();
    store.batch(|s| {
        changes.extend(record_set(s, "p".to_string(), p, num(2.0)));
        changes.extend(record_set(s, "p".to_string(), p, num(3.0)));
    });
    assert_eq!(changes.len(), 2);
    history.append(M, changes);
    assert_eq!(store.get(p).as_number(), Some(3.0));

    let undone = match history.undo_one(no_barrier) {
        UndoOutcome::Applied(entries) => entries,
        _ => panic!("expected Applied"),
    };
    assert_eq!(undone.len(), 1);
    let entry = &undone[0];
    assert_eq!(entry.changes.len(), 2, "batch 里两处写入仍是一条 entry");

    // 调用方闭包：条目内 changes 必须倒序应用（先撤后写的，再撤先写的）。
    for c in entry.changes.iter().rev() {
        store.set(p, c.prev.clone());
    }
    assert_eq!(
        store.get(p).as_number(),
        Some(1.0),
        "prev 链咬合，回到 batch 之前的值——正序应用会错误停在 2.0"
    );

    // redo：正序应用 next，一路推回 3.0。
    let redone = match history.redo_one() {
        UndoOutcome::Applied(entries) => entries,
        _ => panic!("expected Applied"),
    };
    assert_eq!(redone.len(), 1);
    let entry = &redone[0];
    for c in entry.changes.iter() {
        store.set(p, c.next.clone());
    }
    assert_eq!(store.get(p).as_number(), Some(3.0));
}
