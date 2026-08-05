//! 验收 3（017）+「seq 不回收」：游标不在栈顶时写入新 entry → redo 尾被丢弃
//! （`can_redo()` 变 false、多余条目从日志里消失），append 铸出的新 seq
//! 从原来的 next_seq 继续，不重用被丢弃条目占用过的 seq。
//! 钉死接口（重建版本）见 `undo_redo_roundtrip.rs` 顶部注释。

mod common;
use common::*;

use agent_store::{History, Store, UndoOutcome, record_set};

#[derive(Clone, Copy, Debug, PartialEq)]
struct M;

fn no_barrier(_: &M) -> bool {
    false
}

#[test]
fn append_below_top_drops_redo_tail_and_seq_keeps_advancing() {
    let store: Store<TestValue> = Store::new();
    let p = store.create_atom(num(0.0));
    let mut history: History<String, TestValue, M> = History::new();

    // 三条 entry：0->1(seq0), 1->2(seq1), 2->3(seq2)。
    for next in [1.0, 2.0, 3.0] {
        let c = record_set(&store, "p".to_string(), p, num(next)).unwrap();
        history.append(M, vec![c]);
    }
    assert_eq!(history.len(), 3);
    assert_eq!(history.cursor(), 3);

    // undo 两步：游标退到 1（只剩 seq0 那条 applied），redo 尾是 seq1/seq2。
    for _ in 0..2 {
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
    }
    assert_eq!(history.cursor(), 1);
    assert!(history.can_redo());
    assert_eq!(store.get(p).as_number(), Some(1.0));

    // 游标不在顶时写入新值：旧 redo 尾（seq1/seq2）必须被丢弃。
    let c = record_set(&store, "p".to_string(), p, num(99.0)).unwrap();
    let new_seq = history.append(M, vec![c]).unwrap();

    assert_eq!(new_seq, 3, "seq 从原 next_seq 继续，不从游标位置重新数");
    assert_eq!(history.len(), 2, "旧 redo 尾的两条被丢弃，新写入的一条顶上");
    assert!(!history.can_redo());
    assert_eq!(history.cursor(), 2);
    assert_eq!(store.get(p).as_number(), Some(99.0));

    // 再写一条，确认 seq 继续递增、不会撞上被丢弃的 seq1/seq2。
    let c = record_set(&store, "p".to_string(), p, num(100.0)).unwrap();
    let next_seq = history.append(M, vec![c]).unwrap();
    assert_eq!(next_seq, 4, "被丢弃的 seq1/seq2 永不重用");
}
