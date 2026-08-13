//! derived 的重算不产生 `Entry` —— 只有源状态（primitive）进日志。这是「完整状态 =
//! 所有 primitive atom 的值，derived 全部可重算」（STATE-MODEL 开篇）的结构性证明：
//! 2 个 primitive + 1 个 derived，写 primitive 走 `record_set` + `History::append`
//! 落条目；读 derived（无论触发几次重算）绝不落条目。
//!
//! 验收 2。

use crate::common::*;

use einfach_store::{History, Store, record_set};

#[test]
fn derived_recompute_does_not_grow_the_log() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let b = store.create_atom(num(2.0));
    let derived = store.create_derived_ctx(move |args| {
        num(args.get(a).as_number().unwrap() + args.get(b).as_number().unwrap())
    });

    let mut history: History<String, TestValue, ()> = History::new();

    // Reading the derived atom for the first time forces its initial
    // computation. No primitive has been written through record_set yet, so
    // the log must stay empty.
    assert_eq!(store.get(derived).as_number(), Some(3.0));
    assert_eq!(
        history.len(),
        0,
        "computing a derived atom must not log anything"
    );

    // Write primitive A through the recording entry point -- this is the
    // only thing allowed to add an Entry.
    let change_a = record_set(&store, "A".to_string(), a, num(10.0)).expect("1.0 -> 10.0");
    history.append((), vec![change_a]);
    assert_eq!(history.len(), 1);

    // Re-reading `derived` now recomputes it (A changed, so it was stale).
    // The recompute itself must NOT add a second Entry.
    assert_eq!(store.get(derived).as_number(), Some(12.0));
    assert_eq!(
        history.len(),
        1,
        "derived recompute after a primitive write must not add its own Entry"
    );

    // Reading it again with nothing changed (no recompute happens at all
    // this time) is an even stronger case for "no entry" -- confirm it too.
    assert_eq!(store.get(derived).as_number(), Some(12.0));
    assert_eq!(history.len(), 1);

    // Write primitive B -- second (and only second) log entry.
    let change_b = record_set(&store, "B".to_string(), b, num(20.0)).expect("2.0 -> 20.0");
    history.append((), vec![change_b]);
    assert_eq!(history.len(), 2);

    // One more derived recompute pulling in both new values: still no entry.
    assert_eq!(store.get(derived).as_number(), Some(30.0));
    assert_eq!(
        history.len(),
        2,
        "derived recompute must never add an Entry, no matter how many of its \
         primitive dependencies changed"
    );

    // The two entries on the log are exactly the two primitive writes, in
    // order -- derived never sneaked a third one in.
    let keys: Vec<&str> = history
        .entries()
        .flat_map(|e| e.changes.iter().map(|c| c.key.as_str()))
        .collect();
    assert_eq!(keys, vec!["A", "B"]);
}
