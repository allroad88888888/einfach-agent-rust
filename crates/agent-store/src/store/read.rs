//! 喂给 derived 读函数的追踪/免追踪访问口：`ReadArgs` 的 `get`/`peek`/`depend`，和它们
//! 共用的 `Scratch` 暂存区（读期间记的依赖边、还差哪些没算好的 dep、是否已经故障）。
//! 真正驱动求值的显式帧栈状态机在 `eval` 子模块——这里只管"一次 get 该怎么答"，
//! 不管"整棵 atom 树怎么算完"。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ids::AtomId;

use super::eval::{read_atom, seed_primitive};
use super::guards::ReadDepthGuard;
use super::records::{AtomValue, Inner};

/// Tracked/untracked read access handed to derived read functions.
/// `get` is store.ts's tracked `getter`; `peek` is `options.getter`
/// (the noWatch getter) / `getter.peek` — a full read with no edge recorded.
pub struct ReadArgs<'a, V: AtomValue> {
    pub(super) inner: &'a Rc<RefCell<Inner<V>>>,
    pub(super) scratch: &'a RefCell<Scratch>,
}

impl<V: AtomValue> ReadArgs<'_, V> {
    pub fn get(&self, id: AtomId) -> V {
        read_dep(self.inner, self.scratch, id, true)
    }

    /// Untracked read (`noWatchGetter`): computes if needed, records no edge.
    pub fn peek(&self, id: AtomId) -> V {
        read_dep(self.inner, self.scratch, id, false)
    }

    /// Whether this speculative derived read has already faulted because a
    /// dependency still needs to be computed. Callers with observable side
    /// effects can use this to defer them until the retry that can commit.
    pub fn is_faulted(&self) -> bool {
        self.scratch.borrow().faulted
    }

    /// Record a dependency edge on `id` at its current generation WITHOUT
    /// reading its value, tolerating an in-progress (`computing`) atom.
    ///
    /// Both `get` and `peek` PANIC on a `computing` atom (the cross-atom
    /// cycle guard). A runtime cycle in the excel evaluator is intercepted
    /// ABOVE the store — the evaluator's Computing guard returns a sticky
    /// `#CYCLE!` for a point ref to an on-stack address rather than reading
    /// the in-progress peer — but the reader must still wire the reverse
    /// edge so that DISSOLVING the cycle later (an edit that bumps the peer's
    /// generation) re-invalidates it. `depend` is the only primitive that can
    /// record that edge: it reads the peer's current generation directly and
    /// pushes `(id, gen)` into the frame scratch, never touching the value and
    /// never panicking on `computing`. Self-references record no edge, exactly
    /// like the `id == self_id` short-circuit in `read_dep` (store.ts:97-102).
    ///
    /// Recording the pre-commit generation of an on-stack peer is a sound
    /// over-approximation (DV-2): when that peer commits its cycle value its
    /// generation bumps, so this reader is left stale-at-rest and re-derives
    /// to the same `#CYCLE!` on next read (equality-pruned). The only
    /// observable effect is recompute count, never a missed invalidation.
    ///
    /// DIVERGENCE(store.ts): no vanilla counterpart — store.ts lets a cycle
    /// stack-overflow; the excel engine detects cycles itself and needs a way
    /// to wire the reverse edge for cycle dissolution.
    pub fn depend(&self, id: AtomId) {
        let self_id = self.scratch.borrow().self_id;
        if id == self_id {
            return;
        }
        let generation = {
            let inner = self.inner.borrow();
            if !inner.has(id) {
                panic!("atom {:?} not found in store", id);
            }
            inner.record(id).generation
        };
        self.scratch.borrow_mut().record_dep(id, generation);
    }
}

pub(super) struct Scratch {
    /// Read-order `(dep, generation)` pairs recorded by the tracked getter.
    pub(super) deps: Vec<(AtomId, u64)>,
    /// dep → index into `deps` — keeps per-get dedup O(1) so a large fan-in
    /// read (SUM over 100k members) stays linear (codex P1 review P2 #4).
    dep_index: HashMap<AtomId, usize>,
    /// Deps that must be computed before this read can complete (order kept,
    /// set-backed dedup).
    pub(super) needed: Vec<AtomId>,
    needed_set: std::collections::HashSet<AtomId>,
    pub(super) faulted: bool,
    self_id: AtomId,
}

impl Scratch {
    pub(super) fn new(self_id: AtomId) -> Self {
        Scratch {
            deps: Vec::new(),
            dep_index: HashMap::new(),
            needed: Vec::new(),
            needed_set: std::collections::HashSet::new(),
            faulted: false,
            self_id,
        }
    }

    fn record_dep(&mut self, id: AtomId, generation: u64) {
        match self.dep_index.get(&id) {
            Some(&idx) => self.deps[idx].1 = generation,
            None => {
                self.dep_index.insert(id, self.deps.len());
                self.deps.push((id, generation));
            }
        }
    }

    fn record_needed(&mut self, id: AtomId) {
        if self.needed_set.insert(id) {
            self.needed.push(id);
        }
        self.faulted = true;
    }
}

/// Native-stack budget for the recursive half of the DV-3 hybrid: deep
/// enough that hand-written atom graphs always take the faithful recursive
/// path, shallow enough that a 1 MB WASM stack cannot overflow (≈1 KB per
/// nesting level).
pub(super) const READ_RECURSION_BUDGET: usize = 256;

/// The tracked/untracked dep read shared by `ReadArgs::get` / `peek`.
fn read_dep<V: AtomValue>(
    inner: &Rc<RefCell<Inner<V>>>,
    scratch: &RefCell<Scratch>,
    id: AtomId,
    track: bool,
) -> V {
    let self_id = scratch.borrow().self_id;
    {
        let inner_ref = inner.borrow();
        if !inner_ref.has(id) {
            panic!("atom {:?} not found in store", id);
        }
        // store.ts:97-102 — reading yourself inside your own read fn returns
        // the cached state (or init) without registering an edge.
        if id == self_id {
            let rec = inner_ref.record(id);
            return rec
                .value
                .clone()
                .or_else(|| rec.init.clone())
                .expect("self-read of a derived atom before first commit");
        }
        if inner_ref.record(id).computing {
            panic!(
                "circular dependency detected: atom {:?} depends on atom {:?} which is being computed",
                self_id, id
            );
        }
        if inner_ref.is_fresh(id) {
            let rec = inner_ref.record(id);
            let value = rec.value.clone().expect("fresh atom has a value");
            let generation = rec.generation;
            drop(inner_ref);
            if track {
                scratch.borrow_mut().record_dep(id, generation);
            }
            return value;
        }
        // Primitive that was never read: seed from init in place (this is
        // vanilla's readAtom(dep) bottoming out on a primitive).
        if inner_ref.record(id).read_fn.is_none() {
            drop(inner_ref);
            let value = seed_primitive(inner, id);
            if track {
                let generation = inner.borrow().record(id).generation;
                scratch.borrow_mut().record_dep(id, generation);
            }
            return value;
        }
    }
    // Stale/uncomputed derived dep. Two DV-3 sub-paths:
    //
    // 1. Within the recursion budget, compute it inline via a nested
    //    read_atom — this is vanilla's recursive getter pull verbatim, and
    //    every read fn observes correctly-typed dep values. UI-tier atom
    //    graphs never nest deeper than this.
    // 2. Past the budget (deep formula chains), FAULT: record the needed dep
    //    and return a V::null() placeholder. The current run's result and
    //    scratch are discarded; the frame loop computes the dep bottom-up
    //    (iteratively, native stack stays capped at the budget) and re-runs
    //    the faulting read. Read fns must therefore tolerate — not panic on —
    //    unexpected Null from the tracked getter; the engine's evaluator does
    //    so naturally, and the run's output is never committed.
    let depth = inner.borrow().read_depth;
    if depth < READ_RECURSION_BUDGET {
        let _depth_guard = ReadDepthGuard::enter(inner);
        let value = read_atom(inner, id);
        drop(_depth_guard);
        if track {
            let generation = inner.borrow().record(id).generation;
            scratch.borrow_mut().record_dep(id, generation);
        }
        return value;
    }
    scratch.borrow_mut().record_needed(id);
    V::null()
}
