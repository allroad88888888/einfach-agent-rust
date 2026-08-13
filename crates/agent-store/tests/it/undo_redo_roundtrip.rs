//! 验收 1（017）+ 语义「undo→redo 反演」「Applied 顺序 undo 倒序 redo 正序」：
//! 真 Store + TestValue + record_set 全链路。2 个 primitive（p1/p2）+ 1 个 derived
//! （d = p1+p2），两个 turn 各写一条 entry，undo_turn 退回 turn1 末尾状态
//! （primitive 逐值相等、derived 重算一致），redo_turn 完整前进，值精确复原。
//!
//! ## 本文件假设的钉死接口
//!
//! 这是独立测试 agent 对「与实现方完全相同的 API 块」的重建（独测 agent 被禁止看
//! 实现文件，只能拿到主会话对该 API 块的摘要）。按摘要重建为：
//!
//! ```text
//! enum UndoOutcome<K, V, M> {
//!     Applied(Vec<Entry<K, V, M>>),
//!     Blocked { applied: Vec<Entry<K, V, M>>, barrier_seq: u64 },
//!     Nothing,
//! }
//! impl<K, V, M> History<K, V, M> {
//!     fn cursor(&self) -> usize;
//!     fn can_undo(&self) -> bool;
//!     fn can_redo(&self) -> bool;
//!     fn undo_one(&mut self, barrier: impl Fn(&M) -> bool) -> UndoOutcome<K, V, M>;
//!     fn undo_turn(&mut self, same_turn: impl Fn(&M, &M) -> bool, barrier: impl Fn(&M) -> bool) -> UndoOutcome<K, V, M>;
//!     fn redo_one(&mut self) -> UndoOutcome<K, V, M>;
//!     fn redo_turn(&mut self, same_turn: impl Fn(&M, &M) -> bool) -> UndoOutcome<K, V, M>;
//! }
//! ```
//!
//! `UndoOutcome` / `cursor` / `can_undo` / `can_redo` 假定从 `einfach_store` crate 根
//! re-export（`Change`/`Entry`/`History`/`record_set` 已经是这样）。如果实现方选了
//! 别的形状（比如 `Applied` 也是具名字段、或者只从 `einfach_store::history::` 导出），
//! 这是本文件与实现的分歧点，见独测 agent 的最终报告。
//!
//! 值应用（把 `Change.prev`/`next` 写回 store）是调用方的事——`History` 不持有
//! `Store`。下面的 `apply_entries` 就是那个「调用方闭包」：undo 方向 entries 本身
//! 已经是倒序，条目内 `changes` 还要再倒序应用 prev；redo 方向 entries 正序，
//! 条目内 changes 也正序应用 next。

use crate::common::*;

use einfach_store::{AtomId, Change, Entry, History, Store, UndoOutcome, record_set};

#[derive(Clone, Copy, Debug, PartialEq)]
struct M {
    turn: u32,
    barrier: bool,
}

fn meta(turn: u32) -> M {
    M {
        turn,
        barrier: false,
    }
}

fn same_turn(a: &M, b: &M) -> bool {
    a.turn == b.turn
}

fn is_barrier(m: &M) -> bool {
    m.barrier
}

/// 调用方闭包：把 undo/redo 返回的 entries 写回 store。
fn apply_entries(
    store: &Store<TestValue>,
    p1: AtomId,
    p2: AtomId,
    entries: &[Entry<String, TestValue, M>],
    for_undo: bool,
) {
    for entry in entries {
        let ordered: Vec<&Change<String, TestValue>> = if for_undo {
            entry.changes.iter().rev().collect()
        } else {
            entry.changes.iter().collect()
        };
        for c in ordered {
            let atom = match c.key.as_str() {
                "p1" => p1,
                "p2" => p2,
                other => panic!("unknown key: {other}"),
            };
            let v = if for_undo {
                c.prev.clone()
            } else {
                c.next.clone()
            };
            store.set(atom, v);
        }
    }
}

fn expect_applied(outcome: UndoOutcome<String, TestValue, M>) -> Vec<Entry<String, TestValue, M>> {
    match outcome {
        UndoOutcome::Applied(entries) => entries,
        UndoOutcome::Blocked { .. } => panic!("expected Applied, got Blocked"),
        UndoOutcome::Nothing => panic!("expected Applied, got Nothing"),
    }
}

#[test]
fn undo_turn_then_redo_turn_round_trips_primitives_and_derived() {
    let store: Store<TestValue> = Store::new();
    let p1 = store.create_atom(num(1.0));
    let p2 = store.create_atom(num(2.0));
    let d = store.create_derived_ctx(move |args| {
        num(args.get(p1).as_number().unwrap() + args.get(p2).as_number().unwrap())
    });

    let mut history: History<String, TestValue, M> = History::new();

    // Turn 1: 各写一条 entry。
    let c = record_set(&store, "p1".to_string(), p1, num(10.0)).unwrap();
    history.append(meta(1), vec![c]);
    let c = record_set(&store, "p2".to_string(), p2, num(20.0)).unwrap();
    history.append(meta(1), vec![c]);

    assert_eq!(store.get(d).as_number(), Some(30.0));

    // Turn 2: 各写一条 entry。
    let c = record_set(&store, "p1".to_string(), p1, num(100.0)).unwrap();
    let seq_c = history.append(meta(2), vec![c]).unwrap();
    let c = record_set(&store, "p2".to_string(), p2, num(200.0)).unwrap();
    let seq_d = history.append(meta(2), vec![c]).unwrap();

    assert_eq!(store.get(d).as_number(), Some(300.0));
    assert_eq!(history.cursor(), 4);
    assert!(history.can_undo());
    assert!(!history.can_redo());

    // undo_turn 退回 turn2 两条 entry。
    let undone = expect_applied(history.undo_turn(same_turn, is_barrier));
    assert_eq!(undone.len(), 2);
    // undo 倒序：先弹最新的（p2 那条，seq_d），再弹更早的（p1 那条，seq_c）。
    assert_eq!(undone[0].seq, seq_d);
    assert_eq!(undone[1].seq, seq_c);

    apply_entries(&store, p1, p2, &undone, true);

    assert_eq!(store.get(p1).as_number(), Some(10.0));
    assert_eq!(store.get(p2).as_number(), Some(20.0));
    assert_eq!(store.get(d).as_number(), Some(30.0), "derived 重算一致");
    assert_eq!(history.cursor(), 2);
    assert!(history.can_undo());
    assert!(history.can_redo());

    // redo_turn 前进，完整复原 turn2。
    let redone = expect_applied(history.redo_turn(same_turn));
    assert_eq!(redone.len(), 2);
    // redo 正序：先放回更早的（seq_c），再放回更新的（seq_d）。
    assert_eq!(redone[0].seq, seq_c);
    assert_eq!(redone[1].seq, seq_d);

    apply_entries(&store, p1, p2, &redone, false);

    assert_eq!(store.get(p1).as_number(), Some(100.0));
    assert_eq!(store.get(p2).as_number(), Some(200.0));
    assert_eq!(
        store.get(d).as_number(),
        Some(300.0),
        "undo -> redo 精确复原"
    );
    assert_eq!(history.cursor(), 4);
    assert!(!history.can_redo());
}
