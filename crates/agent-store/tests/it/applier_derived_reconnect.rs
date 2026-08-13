//! 019 验收第二条：重建后该 atom 的下游 derived 能正确重算，不是停在旧值。
//!
//! 这里把它拆成一个精确的问题：老的 derived 在 evict 之前就已经被 destroy 掉了
//! （它不在 undo log 里——"derived 不产生 Entry" 是 009 的结构性事实，apply_prev
//! 压根不知道它存在过），所以"重连"不可能是自动的。实测这件事到底怎么发生：
//! 老 derived 的 id 从此失效，想要一个能读到恢复值的 derived，调用方必须在
//! resolve 出来的新 primitive id 上重新建一个。这正是 apply_prev 的验收第三条
//! ("重建走的是与正常创建同一条路径，不是一个特判分支")在 derived 这一侧的样子。

use std::cell::RefCell;
use std::rc::Rc;

use einfach_store::{AtomFamily, AtomId, Entry, Store, apply_prev, record_set};
use crate::common::{TestValue as V, num};

#[test]
fn recreated_primitive_feeds_a_freshly_built_derived_the_old_one_does_not_survive() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));

    let p1 = family
        .borrow_mut()
        .get_or_create("p1".to_string(), || store.create_atom(num(0.0)));

    // 一步写入：0 -> 7。这条 change 的 prev(0.0) 就是待会儿要恢复到的值。
    let change = record_set(&store, "p1".to_string(), p1, num(7.0)).unwrap();
    assert_eq!(change.prev, num(0.0));
    let entry: Entry<String, V, ()> = Entry {
        seq: 0,
        meta: (),
        changes: vec![change],
    };

    // 老 derived：读 p1 翻倍。读一次，建立依赖边。
    let d_old = family.borrow_mut().get_or_create("d".to_string(), || {
        store.create_derived_ctx(move |args| match args.get(p1) {
            V::Number(n) => V::Number(n * 2.0),
            other => other,
        })
    });
    assert_eq!(store.get(d_old), num(14.0));

    // evict：与另一个测试同样的真实 API 行为——先解除依赖（destroy 掉 derived），
    // 再 evict primitive。
    assert!(!family.borrow_mut().evict(&store, &"p1".to_string()));
    assert!(family.borrow_mut().evict(&store, &"d".to_string()));
    assert!(family.borrow_mut().evict(&store, &"p1".to_string()));

    // apply_prev：get-or-create 闭包按需重建 p1。
    let family_for_resolve = family.clone();
    let store_for_resolve = store.clone();
    let mut resolve = move |k: &String| -> AtomId {
        family_for_resolve
            .borrow_mut()
            .get_or_create(k.clone(), || store_for_resolve.create_atom(num(0.0)))
    };
    apply_prev(&store, &mut resolve, std::slice::from_ref(&entry));

    let p1_new = family.borrow().get(&"p1".to_string()).unwrap();
    assert_ne!(p1_new, p1); // 真的是新建的 atom，不是原来那个
    assert_eq!(store.get(p1_new), num(0.0)); // 恢复到 prev

    // 实测真相：老 derived 的 id 已经被 destroy_atom 彻底移除，读它会 panic——
    // 它不会、也不可能被 apply_prev 自动重连，因为它压根没进过 undo log。
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.get(d_old)));
    std::panic::set_hook(prev_hook);
    assert!(
        result.is_err(),
        "老 derived 的 id 应当已经失效——它没有被，也不应该被按需重建"
    );

    // 想要一个能读到恢复值的 derived，调用方必须显式在新 id 上重新建：走的是与
    // 正常创建同一条 create_derived_ctx 路径，不是给"曾经被 evict 过"开的特判分支。
    let d_new = family.borrow_mut().get_or_create("d".to_string(), || {
        store.create_derived_ctx(move |args| match args.get(p1_new) {
            V::Number(n) => V::Number(n * 2.0),
            other => other,
        })
    });
    assert_eq!(store.get(d_new), num(0.0)); // 按恢复值(0.0)重算 = 0.0，不是停在旧值 14.0

    // 依赖图确实接对了：再写一次 p1，新 derived 正常联动，不是停在某个快照上。
    store.set(p1_new, num(5.0));
    assert_eq!(store.get(d_new), num(10.0));
}
