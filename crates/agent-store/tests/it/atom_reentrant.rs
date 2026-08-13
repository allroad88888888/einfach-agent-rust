//! Synchronous re-entrancy (listener setting atoms) and glitch-free propagation.
//! Core behavior #1 and #2: listener re-entry + glitch-free single-derivation-per-batch.

use std::cell::Cell;
use std::rc::Rc;

use crate::common::*;

use einfach_store::Store;

/// atom.test.ts «在订阅回调中更新其他atom时，派生atom应该正常更新»
/// Pins flushPending re-entrancy: a listener that synchronously sets another
/// atom must leave every atom consistent and fire each listener exactly once
/// per change. **Core behavior #1: synchronous re-entrancy.**
#[test]
fn listener_setting_other_atom_keeps_world_consistent() {
    let store = Store::new();
    let count = store.create_atom(num(0.0));
    let double =
        store.create_derived_ctx(move |args| num(args.get(count).as_number().unwrap() * 2.0));
    let triple =
        store.create_derived_ctx(move |args| num(args.get(count).as_number().unwrap() * 3.0));
    let secondary = store.create_atom(num(10.0));

    let secondary_calls = Rc::new(Cell::new(0u32));
    let sc = secondary_calls.clone();
    let secondary_sub = store.sub(secondary, move || sc.set(sc.get() + 1));

    let count_calls = Rc::new(Cell::new(0u32));
    let cc = count_calls.clone();
    let reentrant = store.clone();
    let count_sub = store.sub(count, move || {
        cc.set(cc.get() + 1);
        let current = reentrant.get(count).as_number().unwrap();
        reentrant.set(secondary, num(current + 5.0));
    });

    assert_eq!(store.get(count).as_number(), Some(0.0));
    assert_eq!(store.get(double).as_number(), Some(0.0));
    assert_eq!(store.get(triple).as_number(), Some(0.0));
    assert_eq!(store.get(secondary).as_number(), Some(10.0));
    assert_eq!(count_calls.get(), 0);
    assert_eq!(secondary_calls.get(), 0);

    store.set(count, num(3.0));
    assert_eq!(store.get(count).as_number(), Some(3.0));
    assert_eq!(store.get(double).as_number(), Some(6.0));
    assert_eq!(store.get(triple).as_number(), Some(9.0));
    assert_eq!(store.get(secondary).as_number(), Some(8.0));
    assert_eq!(count_calls.get(), 1);
    assert_eq!(secondary_calls.get(), 1);

    store.set(count, num(7.0));
    assert_eq!(store.get(double).as_number(), Some(14.0));
    assert_eq!(store.get(triple).as_number(), Some(21.0));
    assert_eq!(store.get(secondary).as_number(), Some(12.0));
    assert_eq!(count_calls.get(), 2);
    assert_eq!(secondary_calls.get(), 2);

    store.unsub(count_sub);
    store.set(count, num(10.0));
    assert_eq!(store.get(double).as_number(), Some(20.0));
    assert_eq!(store.get(triple).as_number(), Some(30.0));
    assert_eq!(store.get(secondary).as_number(), Some(12.0));
    assert_eq!(count_calls.get(), 2);
    assert_eq!(secondary_calls.get(), 2);

    store.unsub(secondary_sub);
}

/// atom.test.ts «在订阅回调中更新派生atom的依赖时，派生atom应该正常更新»
/// Complements re-entrancy: derived atom must recompute exactly once when
/// its dependency is updated from a listener. **Reinforces core behavior #1.**
#[test]
fn listener_setting_dep_updates_derived_once() {
    let store = Store::new();
    let base = store.create_atom(num(1.0));
    let derived =
        store.create_derived_ctx(move |args| num(args.get(base).as_number().unwrap() * 10.0));
    let control = store.create_atom(num(0.0));

    let base_calls = Rc::new(Cell::new(0u32));
    let derived_calls = Rc::new(Cell::new(0u32));
    let control_calls = Rc::new(Cell::new(0u32));
    let bc = base_calls.clone();
    let dc = derived_calls.clone();
    let xc = control_calls.clone();

    let base_sub = store.sub(base, move || bc.set(bc.get() + 1));
    let derived_sub = store.sub(derived, move || dc.set(dc.get() + 1));
    let reentrant = store.clone();
    let control_sub = store.sub(control, move || {
        xc.set(xc.get() + 1);
        let current = reentrant.get(control).as_number().unwrap();
        reentrant.set(base, num(current * 2.0));
    });

    assert_eq!(store.get(derived).as_number(), Some(10.0));

    store.set(control, num(5.0));
    assert_eq!(store.get(control).as_number(), Some(5.0));
    assert_eq!(store.get(base).as_number(), Some(10.0));
    assert_eq!(store.get(derived).as_number(), Some(100.0));
    assert_eq!(control_calls.get(), 1);
    assert_eq!(base_calls.get(), 1);
    assert_eq!(derived_calls.get(), 1);

    store.unsub(control_sub);
    store.set(control, num(8.0));
    assert_eq!(store.get(base).as_number(), Some(10.0));
    assert_eq!(store.get(derived).as_number(), Some(100.0));
    assert_eq!(control_calls.get(), 1);
    assert_eq!(base_calls.get(), 1);
    assert_eq!(derived_calls.get(), 1);

    store.unsub(base_sub);
    store.unsub(derived_sub);
}

/// performance.test.ts «应该高效处理大量atom的更新»
/// **Core behavior #2: glitch-free propagation.**
/// A writable atom whose write fn sets 1000 primitives produces exactly ONE
/// notification for a downstream merged derived (renderCount == 1), with all
/// values updated atomically. This asserts that under a single batch, a
/// derived atom recomputes exactly once regardless of how many of its
/// dependencies changed.
#[test]
fn batched_write_of_1000_atoms_publishes_merged_derive_once() {
    let store = Store::new();
    let options: Vec<_> = (0..1000)
        .map(|i| store.create_atom(num(i as f64)))
        .collect();

    let options_for_merge = options.clone();
    let merged = store.create_derived_ctx(move |args| {
        let sum: f64 = options_for_merge
            .iter()
            .map(|&o| args.get(o).as_number().unwrap())
            .sum();
        num(sum)
    });
    let initial: f64 = (0..1000).map(|i| i as f64).sum();
    assert_eq!(store.get(merged).as_number(), Some(initial));

    let render_count = Rc::new(Cell::new(0u32));
    let rc = render_count.clone();
    store.sub(merged, move || rc.set(rc.get() + 1));

    let options_for_write = options.clone();
    let update_all = store.create_writable(
        |_args| num(0.0),
        move |args, _value| {
            for (i, &o) in options_for_write.iter().enumerate() {
                args.set(o, num((i + 1000) as f64));
            }
        },
    );

    store.set(update_all, num(1.0));

    let updated: f64 = (0..1000).map(|i| (i + 1000) as f64).sum();
    assert_eq!(store.get(merged).as_number(), Some(updated));
    assert_eq!(render_count.get(), 1, "batched write must publish once");
    assert_eq!(store.get(options[0]).as_number(), Some(1000.0));
    assert_eq!(store.get(options[999]).as_number(), Some(1999.0));
}
