//! Complex dependency management: complex networks, dynamic deps, writable atoms, peek, notifications.
//! Ported from upstream atom.complex.test.ts and noWatchGetter.test.ts.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

mod common;
use common::*;

use agent_store::Store;

/// atom.complex.test.ts «应该正确处理复杂的依赖网络» (exact closed-form values)
#[test]
fn complex_dependency_network() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let b = store.create_atom(num(2.0));
    let c = store.create_atom(num(3.0));

    let ab_sum = store.create_derived_ctx(move |args| num(args.get(a).as_number().unwrap() + args.get(b).as_number().unwrap()));
    let bc_product = store.create_derived_ctx(move |args| num(args.get(b).as_number().unwrap() * args.get(c).as_number().unwrap()));
    let complex = store.create_derived_ctx(move |args| {
        num(args.get(ab_sum).as_number().unwrap() * args.get(bc_product).as_number().unwrap() - args.get(a).as_number().unwrap())
    });

    assert_eq!(store.get(ab_sum).as_number(), Some(3.0));
    assert_eq!(store.get(bc_product).as_number(), Some(6.0));
    assert_eq!(store.get(complex).as_number(), Some(17.0));

    store.set(a, num(4.0));
    assert_eq!(store.get(ab_sum).as_number(), Some(6.0));
    assert_eq!(store.get(bc_product).as_number(), Some(6.0));
    assert_eq!(store.get(complex).as_number(), Some(32.0));

    store.set(b, num(5.0));
    assert_eq!(store.get(ab_sum).as_number(), Some(9.0));
    assert_eq!(store.get(bc_product).as_number(), Some(15.0));
    assert_eq!(store.get(complex).as_number(), Some(131.0));

    store.set(c, num(6.0));
    assert_eq!(store.get(ab_sum).as_number(), Some(9.0));
    assert_eq!(store.get(bc_product).as_number(), Some(30.0));
    assert_eq!(store.get(complex).as_number(), Some(266.0));
}

/// atom.complex.test.ts «应该处理基于条件的动态依赖» — the full branch-switch
/// choreography, pinning clearDependencies-per-re-read (via commit diff).
#[test]
fn dynamic_deps_switch_branch() {
    let store = Store::new();
    let condition = store.create_atom(TestValue::Boolean(true));
    let a = store.create_atom(num(5.0));
    let b = store.create_atom(num(10.0));

    let dynamic = store.create_derived_ctx(move |args| {
        if args.get(condition).as_bool().unwrap_or(false) {
            args.get(a)
        } else {
            args.get(b)
        }
    });

    assert_eq!(store.get(dynamic).as_number(), Some(5.0));

    store.set(condition, TestValue::Boolean(false));
    assert_eq!(store.get(dynamic).as_number(), Some(10.0));

    store.set(b, num(20.0));
    assert_eq!(store.get(dynamic).as_number(), Some(20.0));

    store.set(condition, TestValue::Boolean(true));
    assert_eq!(store.get(dynamic).as_number(), Some(5.0));

    store.set(a, num(15.0));
    assert_eq!(store.get(dynamic).as_number(), Some(15.0));
}

/// atom.complex.test.ts «具有多层写入的可写派生atom»
/// TWIN-ADAPT: name split by ASCII '-' instead of CJK slicing.
#[test]
fn multi_layer_writable_writes() {
    let store = Store::new();
    let first = store.create_atom(txt("Zhang"));
    let last = store.create_atom(txt("San"));

    let full = store.create_writable(
        move |args| {
            txt(&format!(
                "{}-{}",
                args.get(first).as_text().unwrap(),
                args.get(last).as_text().unwrap()
            ))
        },
        move |args, value| {
            let s = value.as_text().unwrap();
            let (f, l) = s.split_once('-').expect("name has one dash");
            args.set(first, txt(f));
            args.set(last, txt(l));
        },
    );
    let greeting = store.create_writable(
        move |args| txt(&format!("Hello, {}!", args.get(full).as_text().unwrap())),
        move |args, value| {
            let s = value.as_text().unwrap();
            let name = s
                .strip_prefix("Hello, ")
                .and_then(|r| r.strip_suffix('!'))
                .expect("greeting shape");
            args.set(full, txt(name));
        },
    );

    assert_eq!(store.get(full).as_text(), Some("Zhang-San".to_string()));
    assert_eq!(store.get(greeting).as_text(), Some("Hello, Zhang-San!".to_string()));

    store.set(full, txt("Li-Si"));
    assert_eq!(store.get(first).as_text(), Some("Li".to_string()));
    assert_eq!(store.get(last).as_text(), Some("Si".to_string()));
    assert_eq!(store.get(greeting).as_text(), Some("Hello, Li-Si!".to_string()));

    store.set(greeting, txt("Hello, Wang-Wu!"));
    assert_eq!(store.get(first).as_text(), Some("Wang".to_string()));
    assert_eq!(store.get(last).as_text(), Some("Wu".to_string()));
    assert_eq!(store.get(full).as_text(), Some("Wang-Wu".to_string()));
    assert_eq!(store.get(greeting).as_text(), Some("Hello, Wang-Wu!".to_string()));
}

/// atom.complex.test.ts «带有副作用的写入操作» — write fn reads via
/// WriteArgs::get (store.ts passes raw readAtom as the write getter).
#[test]
fn write_with_side_effects() {
    let store = Store::new();
    let counter = store.create_atom(num(0.0));
    let log: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

    let log_for_write = log.clone();
    let logging = store.create_writable(
        move |args| args.get(counter),
        move |args, value| {
            let prev = args.get(counter).as_number().unwrap();
            let next = value.as_number().unwrap();
            log_for_write
                .borrow_mut()
                .push(format!("Counter changed: {} -> {}", prev, next));
            args.set(counter, num(next));
        },
    );

    assert_eq!(store.get(logging).as_number(), Some(0.0));
    assert_eq!(log.borrow().len(), 0);

    store.set(logging, num(5.0));
    assert_eq!(store.get(logging).as_number(), Some(5.0));
    assert_eq!(log.borrow().as_slice(), ["Counter changed: 0 -> 5"]);

    store.set(logging, num(8.0));
    assert_eq!(store.get(logging).as_number(), Some(8.0));
    assert_eq!(
        log.borrow().as_slice(),
        ["Counter changed: 0 -> 5", "Counter changed: 5 -> 8"]
    );
}

/// atom.complex.test.ts «应该只在值真正改变时通知订阅者»
/// DV-2 ADAPT: vanilla uses Object.is (reference identity) so a structurally
/// equal but fresh object notifies; Rust PartialEq prunes it. Asserted here
/// as the documented divergence: structural-equal replacement → NO notify;
/// genuine change → notify.
#[test]
fn notify_only_on_real_change_partial_eq() {
    let store = Store::new();
    let data = store.create_atom(txt("count:0"));
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    store.sub(data, move || c.set(c.get() + 1));

    store.set(data, txt("count:0"));
    assert_eq!(calls.get(), 0);

    store.set(data, txt("count:1"));
    assert_eq!(calls.get(), 1);

    store.set(data, txt("count:1"));
    assert_eq!(calls.get(), 1);
}

/// atom.complex.test.ts «间接依赖更新时的选择性通知»
#[test]
fn selective_notification_on_indirect_deps() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let b = store.create_atom(num(2.0));

    let derived_a = store.create_derived_ctx(move |args| num(args.get(a).as_number().unwrap() * 2.0));
    let derived_ab = store.create_derived_ctx(move |args| num(args.get(a).as_number().unwrap() + args.get(b).as_number().unwrap()));

    let calls_a = Rc::new(Cell::new(0u32));
    let calls_ab = Rc::new(Cell::new(0u32));
    let ca = calls_a.clone();
    let cab = calls_ab.clone();
    store.sub(derived_a, move || ca.set(ca.get() + 1));
    store.sub(derived_ab, move || cab.set(cab.get() + 1));

    store.set(a, num(3.0));
    assert_eq!(calls_a.get(), 1);
    assert_eq!(calls_ab.get(), 1);

    store.set(b, num(4.0));
    assert_eq!(calls_a.get(), 1);
    assert_eq!(calls_ab.get(), 2);
}

/// noWatchGetter.test.ts «不监听atom的变化»
#[test]
fn peek_does_not_track() {
    let store = Store::new();
    let a = store.create_atom(num(0.0));
    let b = store.create_derived_ctx(move |args| num(args.peek(a).as_number().unwrap() + 1.0));
    assert_eq!(store.get(b).as_number(), Some(1.0));
    store.set(a, num(10.0));
    assert_eq!(store.get(b).as_number(), Some(1.0));
}

/// noWatchGetter.test.ts «不监听atom的变化-再嵌套一层»
#[test]
fn peek_of_tracked_derived_does_not_track() {
    let store = Store::new();
    let a = store.create_atom(num(0.0));
    let b = store.create_derived_ctx(move |args| num(args.get(a).as_number().unwrap() + 1.0));
    let c = store.create_derived_ctx(move |args| num(args.peek(b).as_number().unwrap() + 1.0));
    assert_eq!(store.get(c).as_number(), Some(2.0));
    store.set(a, num(10.0));
    assert_eq!(store.get(c).as_number(), Some(2.0));
}

/// noWatchGetter.test.ts «不监听atom的变化-再嵌套一层-再设置一次»
#[test]
fn peek_nested_with_extra_dep_set() {
    let store = Store::new();
    let a = store.create_atom(num(0.0));
    let aa = store.create_atom(num(3.0));
    let b = store.create_derived_ctx(move |args| {
        let _ = args.get(aa);
        num(args.get(a).as_number().unwrap() + 1.0)
    });
    let c = store.create_derived_ctx(move |args| num(args.peek(b).as_number().unwrap() + 1.0));
    assert_eq!(store.get(c).as_number(), Some(2.0));
    store.set(a, num(10.0));
    store.set(aa, num(10.0));
    assert_eq!(store.get(c).as_number(), Some(2.0));
}
