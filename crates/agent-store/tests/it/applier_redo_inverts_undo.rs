//! 019 验收覆盖第四条：apply_prev 之后对同一批 entries 跑 apply_next（redo 序）
//! 必须把值全部带回 undo 之前。
//!
//! 顺序契约是不对称的：apply_prev 要按"新的先来"喂 entries（撤销顺序），
//! apply_next 要按"旧的先来"喂同一批 entries（重放顺序）——这正是
//! `docs/STATE-MODEL.md` 与 017 `UndoOutcome` 文档说的"undo 按 seq 倒序、
//! redo 按正序"在 applier 这一层的样子。函数本身不重排 entries，顺序是调用方的责任。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{AtomFamily, AtomId, Change, Entry, Store, apply_next, apply_prev};
use common::{TestValue as V, num};

#[test]
fn apply_next_replayed_in_redo_order_restores_the_pre_undo_state() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));
    let a = family
        .borrow_mut()
        .get_or_create("a".to_string(), || store.create_atom(num(0.0)));
    let b = family
        .borrow_mut()
        .get_or_create("b".to_string(), || store.create_atom(num(0.0)));

    // 两个历史步骤，按发生的先后顺序：
    //   entry_old: a 0->1, b 0->10
    //   entry_new: a 1->2, b 10->20
    let entry_old: Entry<String, V, ()> = Entry {
        seq: 0,
        meta: (),
        changes: vec![
            Change {
                key: "a".to_string(),
                prev: num(0.0),
                next: num(1.0),
            },
            Change {
                key: "b".to_string(),
                prev: num(0.0),
                next: num(10.0),
            },
        ],
    };
    let entry_new: Entry<String, V, ()> = Entry {
        seq: 1,
        meta: (),
        changes: vec![
            Change {
                key: "a".to_string(),
                prev: num(1.0),
                next: num(2.0),
            },
            Change {
                key: "b".to_string(),
                prev: num(10.0),
                next: num(20.0),
            },
        ],
    };

    // 当前状态就是两步都发生之后的样子。
    store.set(a, num(2.0));
    store.set(b, num(20.0));

    let family_for_resolve = family.clone();
    let store_for_resolve = store.clone();
    let mut resolve = move |k: &String| -> AtomId {
        family_for_resolve
            .borrow_mut()
            .get_or_create(k.clone(), || store_for_resolve.create_atom(num(0.0)))
    };

    // undo 序：新的先来。
    apply_prev(
        &store,
        &mut resolve,
        &[entry_new.clone(), entry_old.clone()],
    );
    assert_eq!(store.get(a), num(0.0));
    assert_eq!(store.get(b), num(0.0));

    // redo 序：同一批 entries，旧的先来——apply_next 的顺序契约与 apply_prev 相反。
    apply_next(
        &store,
        &mut resolve,
        &[entry_old.clone(), entry_new.clone()],
    );

    // 值全部回到 undo 之前。
    assert_eq!(store.get(a), num(2.0));
    assert_eq!(store.get(b), num(20.0));
}
