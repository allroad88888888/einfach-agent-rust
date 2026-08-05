//! Divergence-primitive tests for `ReadArgs::depend`.
//!
//! `depend` has NO core/core twin — it is the DV row that lets an
//! atom-delegated formula wire a reverse dependency edge on an in-progress
//! (`computing`) peer without reading its value, so a later edit that bumps
//! that peer's generation re-invalidates the depender (cycle dissolution).
//! These tests pin its three contract properties directly. See the doc
//! comment on `ReadArgs::depend` in store.rs and the cycle-semantics design.
//!
//! This mechanism handles **core behavior #3: circular dependency detection**
//! by allowing forward edges to be established without reading, preventing
//! deadlock in cross-atom cycles while maintaining correctness.

use std::cell::Cell;
use std::rc::Rc;

mod common;
use common::*;

use agent_store::Store;

/// `depend` records a re-invalidating edge WITHOUT reading the dependency's
/// value: the depender never calls `get(src)`, yet a later `set(src, ...)`
/// still forces it to re-derive (edge generation mismatch).
#[test]
fn depend_records_reinvalidating_edge_without_reading() {
    let store = Store::new();
    let src = store.create_atom(num(1.0));

    // The read_fn depends on `src` but NEVER reads its value; it returns a
    // monotonic run counter so we can observe re-derivation.
    let runs = Rc::new(Cell::new(0u32));
    let runs_inner = runs.clone();
    let dep = store.create_derived_ctx(move |args| {
        args.depend(src);
        let next = runs_inner.get() + 1;
        runs_inner.set(next);
        num(next as f64)
    });

    // First read: runs once, returns 1, edge (src, gen) recorded.
    assert_eq!(store.get(dep).as_number(), Some(1.0));
    assert_eq!(runs.get(), 1);

    // Second read with src unchanged: fresh, NOT re-run.
    assert_eq!(store.get(dep).as_number(), Some(1.0));
    assert_eq!(runs.get(), 1);

    // Bump src's generation: the recorded edge is now stale.
    store.set(src, num(2.0));

    // Third read: stale edge -> re-derive, even though we never read src.
    assert_eq!(store.get(dep).as_number(), Some(2.0));
    assert_eq!(runs.get(), 2);
}

/// `depend` tolerates a `computing` peer: both `get` and `peek` PANIC on an
/// in-progress atom (the cross-atom cycle guard), but `depend` must record
/// the edge without panicking. `outer` is mid-read (computing) when `inner`
/// calls `depend(outer)`. This is essential for **circular dependency handling**.
#[test]
fn depend_tolerates_computing_peer_without_panicking() {
    let store = Store::new();

    let outer_holder: Rc<Cell<Option<agent_store::AtomId>>> = Rc::new(Cell::new(None));
    let outer_for_inner = outer_holder.clone();
    let inner = store.create_derived_ctx(move |args| {
        // `outer` is on the read stack (computing) at this point.
        if let Some(outer) = outer_for_inner.get() {
            args.depend(outer); // must NOT panic
        }
        num(5.0)
    });
    let outer =
        store.create_derived_ctx(move |args| num(args.get(inner).as_number().unwrap() + 1.0));
    outer_holder.set(Some(outer));

    // Reading `outer` drives `inner`, which calls `depend` on the still-
    // computing `outer`. No panic; value flows through.
    assert_eq!(store.get(outer).as_number(), Some(6.0));
}

/// A self-reference records no edge, exactly like the short-circuit in
/// `read_dep`: the atom must not end up as its own dependent.
#[test]
fn depend_on_self_records_no_edge() {
    let store = Store::new();

    let id_holder: Rc<Cell<Option<agent_store::AtomId>>> = Rc::new(Cell::new(None));
    let id_for_fn = id_holder.clone();
    let selfish = store.create_derived_ctx(move |args| {
        if let Some(me) = id_for_fn.get() {
            args.depend(me); // self -> no edge
        }
        num(42.0)
    });
    id_holder.set(Some(selfish));

    assert_eq!(store.get(selfish).as_number(), Some(42.0));
    // No self-edge: `selfish` is nobody's dependent, including its own.
    assert_eq!(store.debug_dependent_count(selfish), 0);
}

#[test]
fn reverse_dependents_enumerates_committed_back_deps_transitively() {
    let store = Store::new();
    let src = store.create_atom(num(1.0));
    let other = store.create_atom(num(10.0));

    let first = store.create_derived_ctx(move |args| num(args.get(src).as_number().unwrap() + 1.0));
    let second =
        store.create_derived_ctx(move |args| num(args.get(first).as_number().unwrap() * 2.0));
    let sibling = store.create_derived_ctx(move |args| {
        num(args.get(src).as_number().unwrap() + args.get(other).as_number().unwrap())
    });
    let isolated = store.create_derived_ctx(move |args| num(args.get(other).as_number().unwrap()));

    assert_eq!(store.get(second).as_number(), Some(4.0));
    assert_eq!(store.get(sibling).as_number(), Some(11.0));
    assert_eq!(store.get(isolated).as_number(), Some(10.0));

    let from_src = store.reverse_dependents(&[src]);
    assert!(from_src.contains(&first));
    assert!(from_src.contains(&second));
    assert!(from_src.contains(&sibling));
    assert!(!from_src.contains(&src));
    assert!(!from_src.contains(&isolated));

    assert_eq!(store.reverse_dependents(&[first]), vec![second]);
}
