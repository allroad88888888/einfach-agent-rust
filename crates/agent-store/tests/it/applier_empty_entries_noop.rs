//! 019 验收覆盖第六条：空 entries 时 apply_prev / apply_next 都不该动 store、
//! 不该碰 resolve，更不能 panic。
//!
//! 覆盖两种"空"：整个 entries 切片是空的；以及 entries 非空但其中一条 entry 自己的
//! changes 是空的（History::append 结构上不会产生这种条目——009/017 的验收就是
//! "空 changes 不落条目"——但 applier 的契约只看 `&[Entry<..>]` 这个形状本身，
//! 防御性地把这个边界也钉住）。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{apply_next, apply_prev, AtomFamily, AtomId, Entry, Store};
use common::{num, TestValue as V};

fn counting_resolve(
    family: Rc<RefCell<AtomFamily<String>>>,
    store: Store<V>,
    calls: Rc<RefCell<u32>>,
) -> impl FnMut(&String) -> AtomId {
    move |k: &String| -> AtomId {
        *calls.borrow_mut() += 1;
        family
            .borrow_mut()
            .get_or_create(k.clone(), || store.create_atom(num(0.0)))
    }
}

#[test]
fn empty_entries_slice_is_a_pure_noop_for_both_directions() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));
    let calls = Rc::new(RefCell::new(0u32));
    let mut resolve = counting_resolve(family.clone(), store.clone(), calls.clone());

    let before = store.debug_total_atom_count();
    let entries: [Entry<String, V, ()>; 0] = [];

    apply_prev(&store, &mut resolve, &entries);
    apply_next(&store, &mut resolve, &entries);

    assert_eq!(*calls.borrow(), 0, "空 entries 不该碰 resolve");
    assert_eq!(store.debug_total_atom_count(), before, "空 entries 不该动 store");
}

#[test]
fn an_entry_with_no_changes_is_also_a_noop() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));
    let calls = Rc::new(RefCell::new(0u32));
    let mut resolve = counting_resolve(family.clone(), store.clone(), calls.clone());

    let empty_entry: Entry<String, V, ()> = Entry { seq: 0, meta: (), changes: vec![] };
    let before = store.debug_total_atom_count();

    apply_prev(&store, &mut resolve, std::slice::from_ref(&empty_entry));
    apply_next(&store, &mut resolve, std::slice::from_ref(&empty_entry));

    assert_eq!(*calls.borrow(), 0, "没有 change 的 entry 不该碰 resolve");
    assert_eq!(store.debug_total_atom_count(), before, "没有 change 的 entry 不该动 store");
}
