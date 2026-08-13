//! `History::append` 本身的日志语义：一次 append 铸一个 seq；空 `changes` 不落条目；
//! 多个 `Change` 攒进同一次 append = 一条 `Entry`（"一次 batch = 一个 undo 步"的约定，
//! STATE-MODEL §写入必须收口）；`len` / `is_empty` / `last` / `entries()` 互相一致。
//!
//! 不依赖 `Store` —— `Change` 的字段都是 `pub`，这里直接构造，测的是 `History`
//! 数据结构本身的契约，不是写入路径（那是 `history_record_set.rs` 的事）。
//!
//! 验收 5（batch 语义示范）、验收 6（空 changes append）。

use crate::common::*;

use einfach_store::{Change, History};

fn change(key: &str, prev: TestValue, next: TestValue) -> Change<String, TestValue> {
    Change {
        key: key.to_string(),
        prev,
        next,
    }
}

/// 验收 6：空 `changes` 的 append 返回 `None`，且不落条目——`len` 不变。
#[test]
fn empty_changes_append_returns_none_and_does_not_grow_log() {
    let mut history: History<String, TestValue, ()> = History::new();
    assert!(history.is_empty());

    let result = history.append((), Vec::new());
    assert!(result.is_none(), "empty changes must not mint a seq");
    assert_eq!(history.len(), 0);
    assert!(history.is_empty());
    assert!(history.last().is_none());
}

/// 验收 5：一次逻辑事务写 3 个 atom -> 3 个 `Change` 攒进**一次** append -> History
/// 恰好 1 条，`changes.len() == 3`。这就是"一次 batch = 一个 undo 步"的字面意思：
/// 条目数只随 append 调用次数走，跟单次 append 里塞了几个 Change 无关。
#[test]
fn one_batch_of_three_changes_is_one_entry() {
    let mut history: History<String, TestValue, ()> = History::new();

    let batch = vec![
        change("root/agent_a", num(0.0), num(1.0)),
        change("root/agent_b", num(0.0), num(2.0)),
        change("root/agent_c", num(0.0), num(3.0)),
    ];
    let seq = history.append((), batch).expect("non-empty batch appends");

    assert_eq!(
        history.len(),
        1,
        "one append call = one undo step, regardless of how many changes it carries"
    );
    let entry = history.last().expect("just appended");
    assert_eq!(entry.seq, seq);
    assert_eq!(entry.changes.len(), 3);
    assert_eq!(entry.changes[0].key, "root/agent_a");
    assert_eq!(entry.changes[1].key, "root/agent_b");
    assert_eq!(entry.changes[2].key, "root/agent_c");
}

/// General proof that `seq` is strictly increasing across successive appends
/// (both the "两次 append 后 History 2 条、seq 递增" half of acceptance 1 and a
/// standalone contract of `History` itself, independent of how the `Change`s
/// were produced).
#[test]
fn seq_strictly_increases_across_appends() {
    let mut history: History<String, TestValue, ()> = History::new();

    let seq1 = history
        .append((), vec![change("A", num(1.0), num(2.0))])
        .unwrap();
    let seq2 = history
        .append((), vec![change("A", num(2.0), num(3.0))])
        .unwrap();
    let seq3 = history
        .append((), vec![change("B", num(0.0), num(1.0))])
        .unwrap();

    assert!(seq1 < seq2, "seq must strictly increase");
    assert!(seq2 < seq3, "seq must strictly increase");
    assert_eq!(history.len(), 3);

    let seqs: Vec<u64> = history.entries().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![seq1, seq2, seq3]);
}

/// An empty append sandwiched between two real ones must not land an entry,
/// even though it's not the first or last call.
#[test]
fn empty_batch_between_real_batches_does_not_land_an_entry() {
    let mut history: History<String, TestValue, ()> = History::new();
    let seq1 = history
        .append((), vec![change("A", num(1.0), num(2.0))])
        .unwrap();
    assert!(history.append((), Vec::new()).is_none());
    let seq2 = history
        .append((), vec![change("A", num(2.0), num(3.0))])
        .unwrap();

    assert_eq!(
        history.len(),
        2,
        "the empty append in the middle must not have landed an entry"
    );
    // Whether seq2 == seq1 + 1 exactly is an implementation choice the pinned
    // interface doesn't promise (append() only promises "no entry" for an
    // empty batch, not "no seq consumed"). Only assert what's guaranteed:
    // strict monotonic increase across the entries that DID land.
    assert!(seq2 > seq1);
}
