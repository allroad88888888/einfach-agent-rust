//! Deep recursion budget, panic safety, and performance characteristics.
//! **Core behavior #4: 256-depth recursion budget under deep chains.**
//! Plus panic unwinding safety (unwind guards) and O(N) vs O(N²) analysis.

use std::cell::Cell;
use std::rc::Rc;

mod common;
use common::*;

use agent_store::Store;

/// Helper: build a chain of `depth` derived atoms, each incrementing by 1.
/// The tail reads via args.get, and chains past the recursion budget will
/// cross the FAULT path where to-be-computed values read back as null.
fn build_chain(store: &Store<TestValue>, depth: usize) -> (agent_store::AtomId, agent_store::AtomId) {
    let head = store.create_atom(num(0.0));
    let mut prev = head;
    for _ in 0..depth {
        let link = store
            .create_derived_ctx(move |args| num(args.get(prev).as_number().unwrap_or(0.0) + 1.0));
        prev = link;
    }
    (head, prev)
}

/// Core behavior #4: Cold pull of a 100k-deep chain must not overflow the
/// stack (DV-3 NeedsDep frame loop) and must complete each link exactly once.
/// The chain is evaluated iteratively, not recursively.
#[test]
fn chain_100k_cold_read_is_iterative_and_linear() {
    let store = Store::new();
    let depth = 100_000;
    let (_, tail) = build_chain(&store, depth);

    let before = store.debug_recompute_count();
    assert_eq!(store.get(tail).as_number(), Some(depth as f64));
    let evals = store.debug_recompute_count() - before;
    assert_eq!(evals, depth, "each link completes exactly once");

    // Re-read: fully cached.
    let before = store.debug_recompute_count();
    assert_eq!(store.get(tail).as_number(), Some(depth as f64));
    assert_eq!(store.debug_recompute_count() - before, 0);
}

/// Head write into a fully-hydrated 100k chain: iterative
/// dependencies_change, one recompute per link, one visit per link.
#[test]
fn chain_100k_head_write_flush_is_iterative_and_linear() {
    let store = Store::new();
    let depth = 100_000;
    let (head, tail) = build_chain(&store, depth);
    let _ = store.get(tail);
    store.flush();

    let evals_before = store.debug_recompute_count();
    let visits_before = store.debug_flush_visit_count();
    store.set(head, num(1.0));
    assert_eq!(
        store.debug_recompute_count() - evals_before,
        depth,
        "each link re-derives exactly once during the flush"
    );
    // Closed form 2N−1, straight from vanilla flushPending mechanics:
    // round 1 drains [head] and the dependencies_change walk re-derives all
    // N links (N visits, each bumping write_seq so settled-memo stamps go
    // stale); round 2 drains the N re-derived links and each walk revalidates
    // its single dependent (N−1 visits, all pruned as unchanged, 0 evals).
    assert_eq!(
        store.debug_flush_visit_count() - visits_before,
        2 * depth - 1,
        "N re-derive visits + (N-1) second-round revalidation visits"
    );
    assert_eq!(store.get(tail).as_number(), Some((depth + 1) as f64));
}

/// Panic safety: a panicking batch body must not leave batch_depth elevated
/// (or every later set would defer forever).
#[test]
fn batch_panic_does_not_leak_depth() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let calls = Rc::new(Cell::new(0u32));
    let c = calls.clone();
    store.sub(a, move || c.set(c.get() + 1));

    let store_for_panic = store.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store_for_panic.batch(|s| {
            s.set(a, num(2.0));
            panic!("intentional panic in batch");
        });
    }));
    assert!(result.is_err());

    store.set(a, num(99.0));
    assert_eq!(store.get(a).as_number(), Some(99.0));
    assert!(calls.get() >= 1, "subscriber must fire after the panic");
}

/// Panic safety: a panicking read fn must not leave the computing flag set
/// (false cycle panics) nor the nesting counter elevated (silent fault-path degradation).
#[test]
fn read_fn_panic_does_not_poison_computing_state() {
    let store = Store::new();
    let a = store.create_atom(num(1.0));
    let boom = store.create_derived_ctx(move |args| {
        let _ = args.get(a);
        panic!("intentional panic in read fn");
    });
    let observer = store.create_derived_ctx(move |args| args.get(boom));

    let store_for_panic = store.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = store_for_panic.get(observer);
    }));
    assert!(result.is_err());

    // A fresh graph on the same store must work cleanly afterwards.
    let b = store.create_atom(num(10.0));
    let c = store.create_derived_ctx(move |args| num(args.get(b).as_number().unwrap() * 2.0));
    assert_eq!(store.get(c).as_number(), Some(20.0));
    store.set(b, num(5.0));
    assert_eq!(store.get(c).as_number(), Some(10.0));
}

/// Panic safety: a panicking write fn must not leave the write-cycle guard armed.
#[test]
fn write_fn_panic_does_not_poison_setting_guard() {
    let store = Store::new();
    let base = store.create_atom(num(0.0));
    let strict = store.create_writable(
        move |args| args.get(base),
        move |args, value| {
            if value.as_number().unwrap() < 0.0 {
                panic!("negative writes rejected");
            }
            args.set(base, value);
        },
    );

    let store_for_panic = store.clone();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        store_for_panic.set(strict, num(-1.0));
    }));
    assert!(result.is_err());

    // The guard must be clear: a valid write goes through.
    store.set(strict, num(7.0));
    assert_eq!(store.get(base).as_number(), Some(7.0));
}

/// Performance: large fan-in stays linear (codex P1 review: per-get dedup and
/// commit diff must be O(1)/O(N), not O(N²)): a 20k-member aggregate re-derives
/// with exactly one completed run and one visit per member walk.
#[test]
fn large_fan_in_recompute_is_linear() {
    let store = Store::new();
    let members: Vec<_> = (0..20_000)
        .map(|i| store.create_atom(num(i as f64)))
        .collect();
    let members_for_sum = members.clone();
    let sum = store.create_derived_ctx(move |args| {
        let s: f64 = members_for_sum.iter().map(|&m| args.get(m).as_number().unwrap()).sum();
        num(s)
    });
    let expected: f64 = (0..20_000).map(|i| i as f64).sum();
    let before = store.debug_recompute_count();
    assert_eq!(store.get(sum).as_number(), Some(expected));
    assert_eq!(store.debug_recompute_count() - before, 1);

    store.set(members[10_000], num(0.5));
    assert_eq!(store.get(sum).as_number(), Some(expected - 10_000.0 + 0.5));
}

/// Performance: DV-4 settled-memo: a batched write of N primitives feeding ONE
/// shared derived must re-derive it once and validate it O(N) times total —
/// not O(N·deps) (the quadratic C-2 cousin this memo exists to kill).
#[test]
fn settled_memo_bulk_write_into_shared_dependent() {
    let store = Store::new();
    let members: Vec<_> = (0..1000).map(|i| store.create_atom(num(i as f64))).collect();
    let members_for_sum = members.clone();
    let sum = store.create_derived_ctx(move |args| {
        let s: f64 = members_for_sum.iter().map(|&m| args.get(m).as_number().unwrap()).sum();
        num(s)
    });
    let _ = store.get(sum);
    store.flush();

    let evals_before = store.debug_recompute_count();
    let visits_before = store.debug_flush_visit_count();
    store.batch(|s| {
        for &m in &members {
            s.set(m, num(s.get(m).as_number().unwrap() + 1.0));
        }
    });
    let evals = store.debug_recompute_count() - evals_before;
    let visits = store.debug_flush_visit_count() - visits_before;

    assert_eq!(
        store.get(sum).as_number(),
        Some((0..1000).map(|i| i as f64 + 1.0).sum())
    );
    assert_eq!(evals, 1, "shared dependent re-derives once per flush");
    assert_eq!(visits, 1000, "one settled-memo visit per drained root");
}

/// Engine top-level reads may compute a deep cold graph in one `get`.
/// Settling that read must drain its pending entries without replaying the
/// already-computed chain, and the next unrelated write must not inherit them.
#[test]
fn settle_pending_reads_does_not_rewalk_cold_graph() {
    let store = Store::new();
    let (_, tail) = build_chain(&store, 20_000);

    assert_eq!(store.get(tail).as_number(), Some(20_000.0));
    let visits_before = store.debug_flush_visit_count();
    store.settle_pending_reads();
    assert_eq!(
        store.debug_flush_visit_count(),
        visits_before,
        "read settlement must not run dependencies_change"
    );

    let unrelated = store.create_atom(num(0.0));
    store.set(unrelated, num(1.0));
    assert_eq!(
        store.debug_flush_visit_count(),
        visits_before,
        "the next write must not flush pending work from the prior read"
    );
}
