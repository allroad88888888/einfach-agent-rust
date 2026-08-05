//! `record_set` 的写入语义：捕获 prev、构造 `Change`、写入落地；值相等时跳过（不产生
//! `Change`）；`prev` 是写入前当场捕获的，不是某个更早的基线（STATE-MODEL §写入必须
//! 收口，009 开工前修正 §2）。以及 `record_set` 产出的 `Change` 喂进 `History::append`
//! 之后，每次 primitive 写入都落成独立一条 `Entry`，`seq` 严格递增。
//!
//! 验收 1（prev/next 都对 + seq 递增）、验收 3（值相等不进日志）、验收 7（prev 当场
//! 捕获的时序链条）。

use crate::common::*;

use agent_store::{History, Store, record_set};

/// 验收 1：`record_set` 写 primitive A 产出正确的 `Change{prev, next}` 并真的写穿
/// store；再写 B；两次各自 append 之后 History 有 2 条，seq 严格递增。
#[test]
fn record_set_produces_change_with_correct_prev_and_next() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let b = store.create_atom(num(100.0));

    let change_a = record_set(&store, "A".to_string(), a, num(2.0)).expect("1.0 -> 2.0 changed");
    assert_eq!(change_a.key, "A");
    assert_eq!(change_a.prev, num(1.0));
    assert_eq!(change_a.next, num(2.0));
    assert_eq!(
        store.get(a),
        num(2.0),
        "record_set must write the new value through to the store"
    );

    let change_b =
        record_set(&store, "B".to_string(), b, num(200.0)).expect("100.0 -> 200.0 changed");
    assert_eq!(change_b.key, "B");
    assert_eq!(change_b.prev, num(100.0));
    assert_eq!(change_b.next, num(200.0));

    let mut history: History<String, TestValue, ()> = History::new();
    let seq1 = history
        .append((), vec![change_a])
        .expect("non-empty batch appends");
    let seq2 = history
        .append((), vec![change_b])
        .expect("non-empty batch appends");

    assert_eq!(history.len(), 2, "two primitive writes -> two Entries");
    assert!(seq2 > seq1, "seq must strictly increase across appends");

    let entries: Vec<_> = history.entries().collect();
    assert_eq!(entries[0].seq, seq1);
    assert_eq!(entries[1].seq, seq2);
    assert_eq!(entries[0].changes.len(), 1);
    assert_eq!(entries[1].changes.len(), 1);
    assert_eq!(entries[0].changes[0].key, "A");
    assert_eq!(entries[1].changes[0].key, "B");
}

/// 验收 3：值相等（`PartialEq`）的写入不产生 `Change`，store 里的值也不受影响。
#[test]
fn record_set_skips_logging_when_value_is_unchanged() {
    let store = Store::new();
    let a = store.create_atom(num(5.0));

    let result = record_set(&store, "A".to_string(), a, num(5.0));
    assert!(
        result.is_none(),
        "writing an equal value must not produce a Change"
    );
    assert_eq!(store.get(a), num(5.0), "value must be unaffected");
}

/// 验收 7：`prev` 是写入前"当场"捕获的——连续两次 `record_set`，第二次的 `prev`
/// 必须是第一次的 `next`，不是 atom 的原始 init 值。
#[test]
fn record_set_prev_is_captured_at_write_time_not_the_original_init() {
    let store = Store::new();
    let a = store.create_atom(num(1.0)); // A starts at 1, not through record_set.

    let c1 = record_set(&store, "A".to_string(), a, num(2.0)).expect("1.0 -> 2.0");
    assert_eq!(c1.prev, num(1.0));
    assert_eq!(c1.next, num(2.0));

    let c2 = record_set(&store, "A".to_string(), a, num(3.0)).expect("2.0 -> 3.0");
    assert_eq!(
        c2.prev,
        num(2.0),
        "prev must chain off the previous write's next, captured fresh at this call"
    );
    assert_eq!(c2.next, num(3.0));

    assert_eq!(store.get(a), num(3.0));
}
