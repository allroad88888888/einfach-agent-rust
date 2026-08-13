//! Basic store operations: creation, getter, setter, subscription, clear.
//! Ported from upstream store.test.ts.

use std::cell::Cell;
use std::rc::Rc;

use crate::common::*;

use einfach_store::Store;

/// store.test.ts «createStore: 应该创建一个新的store实例»
/// TWIN-ADAPT: vanilla shares one atom object across stores; Rust AtomIds are
/// store-scoped, so isolation is asserted with per-store atoms.
#[test]
fn create_store_instances_are_isolated() {
    let store1 = Store::new();
    let store2 = Store::new();
    let count1 = store1.create_atom(num(0.0));
    let count2 = store2.create_atom(num(0.0));

    store1.set(count1, num(1.0));
    assert_eq!(store1.get(count1).as_number(), Some(1.0));
    assert_eq!(store2.get(count2).as_number(), Some(0.0));
}

/// store.test.ts «store.getter: 应该获取atom的当前值»
#[test]
fn getter_returns_current_value() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    assert_eq!(store.get(count).as_number(), Some(0.0));
    store.set(count, num(1.0));
    assert_eq!(store.get(count).as_number(), Some(1.0));
}

/// store.test.ts «store.getter: 应该计算派生atom的值»
#[test]
fn getter_computes_derived_value() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let double =
        store.create_derived_ctx(move |args| num(args.get(count).as_number().unwrap() * 2.0));
    assert_eq!(store.get(double).as_number(), Some(0.0));
    store.set(count, num(5.0));
    assert_eq!(store.get(double).as_number(), Some(10.0));
}

/// store.test.ts «store.setter: 应该设置可写派生atom的值»
#[test]
fn setter_writes_through_writable_derived() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let double = store.create_writable(
        move |args| num(args.get(count).as_number().unwrap() * 2.0),
        move |args, value| {
            args.set(count, num(value.as_number().unwrap() / 2.0));
        },
    );
    store.set(double, num(10.0));
    assert_eq!(store.get(count).as_number(), Some(5.0));
    assert_eq!(store.get(double).as_number(), Some(10.0));
}

/// store.test.ts «store.sub: 应该订阅atom值的变化» (+unsub half)
#[test]
fn sub_notifies_on_change_and_unsub_stops() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    let sub = store.sub(count, move || c.set(c.get() + 1));

    store.set(count, num(1.0));
    assert_eq!(calls.get(), 1);

    store.unsub(sub);
    store.set(count, num(2.0));
    assert_eq!(calls.get(), 1);
}

/// store.test.ts «store.sub: 应该订阅派生atom值的变化»
#[test]
fn sub_on_derived_notifies_on_dep_change() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let double =
        store.create_derived_ctx(move |args| num(args.get(count).as_number().unwrap() * 2.0));
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    let sub = store.sub(double, move || c.set(c.get() + 1));

    store.set(count, num(1.0));
    assert_eq!(calls.get(), 1);

    store.unsub(sub);
    store.set(count, num(2.0));
    assert_eq!(calls.get(), 1);
}

/// store.test.ts «clear() 丢弃旧世界的 pending 刷新（审计 C-7，防御性）»
/// TWIN-ADAPT: no async setters (DV-1); the old-world pending entry is parked
/// by a bare batched write instead, and DV clear() kills atom definitions
/// (see store.rs clear doc). The protective intent is identical: nothing
/// from before clear() may ghost-recompute or ghost-publish after it.
#[test]
fn clear_discards_old_world_pending() {
    let store = Store::new();
    let base = store.create_atom(num(0.0));
    let derives = Rc::new(Cell::new(0u32));
    let d = derives.clone();
    let derived = store.create_derived_ctx(move |args| {
        d.set(d.get() + 1);
        num(args.get(base).as_number().unwrap() + 1.0)
    });
    let _ = store.get(derived);
    let baseline = derives.get();

    store.batch(|s| {
        s.set(base, num(1.0));
        s.clear();
    });
    store.flush();

    assert_eq!(derives.get(), baseline, "old-world entry ghost-recomputed");
    let base2 = store.create_atom(num(0.0));
    assert_eq!(store.get(base2).as_number(), Some(0.0));
}
