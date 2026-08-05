//! Basic atom operations: primitive atoms, derived atoms, subscriptions.
//! Ported from upstream atom.test.ts (first half).

use std::cell::Cell;
use std::rc::Rc;

mod common;
use common::*;

use agent_store::Store;

/// atom.test.ts «基本功能» (init / update / complex value)
#[test]
fn primitive_atom_basics() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    assert_eq!(store.get(count).as_number(), Some(0.0));
    store.set(count, num(1.0));
    assert_eq!(store.get(count).as_number(), Some(1.0));

    let user = store.create_atom(txt("John:30"));
    assert_eq!(store.get(user).as_text(), Some("John:30".to_string()));
    store.set(user, txt("Jane:25"));
    assert_eq!(store.get(user).as_text(), Some("Jane:25".to_string()));
}

/// atom.test.ts «派生atom: …再派生一个» + «嵌套的派生atom»
#[test]
fn derived_of_derived() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let double =
        store.create_derived_ctx(move |args| num(args.get(count).as_number().unwrap() * 2.0));
    let triple =
        store.create_derived_ctx(move |args| num(args.get(double).as_number().unwrap() * 3.0));
    assert_eq!(store.get(triple).as_number(), Some(0.0));
    store.set(count, num(5.0));
    assert_eq!(store.get(triple).as_number(), Some(30.0));
    assert_eq!(store.get(double).as_number(), Some(10.0));
}

/// atom.test.ts «应该支持多个依赖的派生atom»
#[test]
fn derived_with_multiple_deps() {
    let store = Store::new();
    let first = store.create_atom(txt("John"));
    let last = store.create_atom(txt("Doe"));
    let full = store.create_derived_ctx(move |args| {
        txt(&format!(
            "{} {}",
            args.get(first).as_text().unwrap(),
            args.get(last).as_text().unwrap()
        ))
    });
    assert_eq!(store.get(full).as_text(), Some("John Doe".to_string()));
    store.set(first, txt("Jane"));
    assert_eq!(store.get(full).as_text(), Some("Jane Doe".to_string()));
    store.set(last, txt("Smith"));
    assert_eq!(store.get(full).as_text(), Some("Jane Smith".to_string()));
}

/// atom.test.ts «订阅: 应该在atom值变化时通知订阅者» (two changes then unsub)
#[test]
fn sub_counts_two_changes_then_unsub() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    let sub = store.sub(count, move || c.set(c.get() + 1));

    store.set(count, num(1.0));
    assert_eq!(calls.get(), 1);
    store.set(count, num(2.0));
    assert_eq!(calls.get(), 2);

    store.unsub(sub);
    store.set(count, num(3.0));
    assert_eq!(calls.get(), 2);
}

/// «监听atom的变化» (tracked baseline)
#[test]
fn tracked_getter_follows_changes() {
    let store = Store::new();
    let a = store.create_atom(num(0.0));
    let b = store.create_derived_ctx(move |args| num(args.get(a).as_number().unwrap() + 1.0));
    store.set(a, num(10.0));
    assert_eq!(store.get(b).as_number(), Some(11.0));
}

/// atom.test.ts «应该缓存计算结果直到依赖变化»
#[test]
fn caches_until_dep_change() {
    let store = Store::new();
    let count = store.create_atom(num(1.0));
    let computes = Rc::new(Cell::new(0u32));
    let k = computes.clone();
    let expensive = store.create_derived_ctx(move |args| {
        k.set(k.get() + 1);
        num(args.get(count).as_number().unwrap() * 10.0)
    });

    assert_eq!(store.get(expensive).as_number(), Some(10.0));
    assert_eq!(computes.get(), 1);

    assert_eq!(store.get(expensive).as_number(), Some(10.0));
    assert_eq!(computes.get(), 1);

    store.set(count, num(2.0));
    assert_eq!(store.get(expensive).as_number(), Some(20.0));
    assert_eq!(computes.get(), 2);

    for _ in 0..5 {
        assert_eq!(store.get(expensive).as_number(), Some(20.0));
    }
    assert_eq!(computes.get(), 2);
}

/// «self-set 断开依赖并在依赖变更后保持设置值» — including the value
/// changing TYPE (number → text), which the TestValue enum expresses directly.
#[test]
fn self_set_severs_deps_and_persists() {
    let store = Store::new();
    let base = store.create_atom(num(0.0));
    let derived = store.create_writable(
        move |args| args.get(base),
        |args, value| {
            args.set_self(value);
        },
    );

    assert_eq!(store.get(derived).as_number(), Some(0.0));

    store.set(derived, txt("persisted"));
    store.set(base, num(123.0));

    assert_eq!(store.get(derived).as_text(), Some("persisted".to_string()));
}
