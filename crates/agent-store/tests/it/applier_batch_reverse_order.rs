//! 019 验收覆盖第三条：batch 内同一个 atom 被写两次的 entry，apply_prev 之后的值
//! 必须是这条 entry 第一次写之前的值——changes 必须倒序回滚。
//!
//! 正序回滚会停在中间值（第二笔的 prev），那是一个看起来"动了"、实际没回滚到底
//! 的 bug；本测试直接断言最终值，把这个反例挡在门口。

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{AtomFamily, AtomId, Entry, Store, apply_prev, record_set};
use crate::common::{TestValue as V, num};

#[test]
fn apply_prev_undoes_a_batch_double_write_to_the_same_atom_in_reverse() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));
    let a = family
        .borrow_mut()
        .get_or_create("a".to_string(), || store.create_atom(num(1.0)));

    // 一次 batch 内对同一个 atom 连写两次：1 -> 2 -> 3。
    let mut changes = Vec::new();
    store.batch(|s| {
        changes.extend(record_set(s, "a".to_string(), a, num(2.0)));
        changes.extend(record_set(s, "a".to_string(), a, num(3.0)));
    });
    assert_eq!(changes.len(), 2);
    assert_eq!(
        (changes[0].prev.clone(), changes[0].next.clone()),
        (num(1.0), num(2.0))
    );
    assert_eq!(
        (changes[1].prev.clone(), changes[1].next.clone()),
        (num(2.0), num(3.0))
    );
    assert_eq!(store.get(a), num(3.0));

    let entry: Entry<String, V, ()> = Entry {
        seq: 0,
        meta: (),
        changes,
    };

    let family_for_resolve = family.clone();
    let store_for_resolve = store.clone();
    let mut resolve = move |k: &String| -> AtomId {
        family_for_resolve
            .borrow_mut()
            .get_or_create(k.clone(), || store_for_resolve.create_atom(num(0.0)))
    };
    apply_prev(&store, &mut resolve, std::slice::from_ref(&entry));

    // 倒序回滚：先撤第二笔（写回 prev=2），再撤第一笔（写回 prev=1）——
    // 落点是整个 batch 之前的值 1。
    //
    // 如果实现按正序处理（先撤第一笔写 1，再撤第二笔写 2），会错误地停在 2 ——
    // 那是"这条 entry 第二次写之前的值"，不是"这条 entry 第一次写之前的值"。
    assert_eq!(store.get(a), num(1.0));
}
