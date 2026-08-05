//! 验收 4（017）+ 语义「undo_one 门口撞屏障」「undo_turn 中途撞屏障」
//! 「redo 无屏障」：屏障（`Reversibility::Irreversible` 在这两层测试里的替身，
//! `M.barrier == true`）挡住 undo 越过它，但从不挡 redo。
//! 钉死接口（重建版本）见 `undo_redo_roundtrip.rs` 顶部注释。

use crate::common::*;

use agent_store::{AtomId, History, Store, UndoOutcome, record_set};

#[derive(Clone, Copy, Debug, PartialEq)]
struct M {
    turn: u32,
    barrier: bool,
}

fn m(turn: u32, barrier: bool) -> M {
    M { turn, barrier }
}

fn same_turn(a: &M, b: &M) -> bool {
    a.turn == b.turn
}

fn is_barrier(meta: &M) -> bool {
    meta.barrier
}

fn write(
    store: &Store<TestValue>,
    history: &mut History<String, TestValue, M>,
    atom: AtomId,
    next: f64,
    meta: M,
) -> u64 {
    let c = record_set(store, "p".to_string(), atom, num(next)).unwrap();
    history.append(meta, vec![c]).unwrap()
}

#[test]
fn undo_one_at_the_door_of_a_barrier_is_fully_blocked() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    write(&store, &mut history, p, 1.0, m(1, false));
    let barrier_seq = write(&store, &mut history, p, 2.0, m(1, true));

    let before_cursor = history.cursor();
    match history.undo_one(is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert!(applied.is_empty(), "门口即屏障，applied 必须是空的");
            assert_eq!(bs, barrier_seq);
        }
        _ => panic!("expected Blocked"),
    }
    assert_eq!(history.cursor(), before_cursor, "撞门口屏障，游标不动");
    assert_eq!(store.get(p).as_number(), Some(2.0), "没有任何值被回滚");

    // 再撞一次，结果一样——屏障是永久的墙，不会因为多试一次就松动。
    match history.undo_one(is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert!(applied.is_empty());
            assert_eq!(bs, barrier_seq);
        }
        _ => panic!("expected Blocked"),
    }
    assert_eq!(history.cursor(), before_cursor);
}

#[test]
fn undo_turn_mid_turn_barrier_stops_one_slot_past_it() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    write(&store, &mut history, p, 1.0, m(5, false)); // e0
    let barrier_seq = write(&store, &mut history, p, 2.0, m(5, true)); // e1, 屏障
    write(&store, &mut history, p, 3.0, m(5, false)); // e2
    write(&store, &mut history, p, 4.0, m(5, false)); // e3

    assert_eq!(history.cursor(), 4);

    let applied = match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert_eq!(bs, barrier_seq);
            applied
        }
        _ => panic!("expected Blocked"),
    };
    // 屏障之后的（e3, e2）被弹出，屏障本身（e1）不弹。
    assert_eq!(applied.len(), 2, "applied 恰含屏障之后那些");
    for entry in &applied {
        for c in entry.changes.iter().rev() {
            store.set(p, c.prev.clone());
        }
    }
    assert_eq!(
        store.get(p).as_number(),
        Some(2.0),
        "停在屏障（e1）生效之后的状态"
    );
    assert_eq!(history.cursor(), 2, "游标停在屏障后一格：e0,e1 仍 applied");

    // 再 undo 一次：门口就是屏障本身，彻底不动，和 undo_one 的门口案例同构。
    match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert!(applied.is_empty());
            assert_eq!(bs, barrier_seq);
        }
        _ => panic!("expected Blocked"),
    }
    assert_eq!(history.cursor(), 2);
}

#[test]
fn redo_crosses_a_barrier_sitting_right_below_the_cursor_without_checking_it() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    write(&store, &mut history, p, 1.0, m(5, false)); // e0
    write(&store, &mut history, p, 2.0, m(5, true)); // e1, 屏障
    write(&store, &mut history, p, 3.0, m(5, false)); // e2
    write(&store, &mut history, p, 4.0, m(5, false)); // e3

    // 退到屏障（e1）刚生效的位置：e2,e3 进 redo 尾，e1（屏障）紧邻游标下方。
    match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked { applied, .. } => {
            for entry in &applied {
                for c in entry.changes.iter().rev() {
                    store.set(p, c.prev.clone());
                }
            }
        }
        _ => panic!("expected Blocked"),
    }
    assert_eq!(history.cursor(), 2);
    assert_eq!(store.get(p).as_number(), Some(2.0));

    // redo_turn 的签名里根本没有 barrier 参数——即使紧邻游标下方（cursor-1）的
    // e1 是屏障，也不妨碍把 e2, e3 redo 回来：屏障只挡 undo，不挡 redo。
    let redone = match history.redo_turn(same_turn) {
        UndoOutcome::Applied(entries) => entries,
        _ => panic!("expected Applied"),
    };
    assert_eq!(redone.len(), 2);
    for entry in &redone {
        for c in entry.changes.iter() {
            store.set(p, c.next.clone());
        }
    }
    assert_eq!(history.cursor(), 4);
    assert_eq!(store.get(p).as_number(), Some(4.0), "屏障不挡 redo");
}
