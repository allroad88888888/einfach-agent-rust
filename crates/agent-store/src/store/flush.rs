//! 写入落地后的调度：一次 `set`/`batch` 结束时，把被写脏的 atom 排进 `PendingQueue`，
//! 沿反向依赖边重算下游（`dependencies_change`），最后把值真的变了的那些交给订阅分发。
//! `WriteArgs` 是这条流水线的入口（writable atom 的 write fn 拿到的读写口），
//! `Inner::set_atom_state` 是它的落点（store.ts `setAtomState`）——两者都属于"一次写入
//! 怎么流经 pending 队列变成对外可见的变化"，所以放在一起。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ids::AtomId;

use super::eval::read_atom;
use super::guards::{BatchGuard, SettingGuard};
use super::handle::Store;
use super::records::{AtomValue, Inner};
use super::subscribe::publish_atom;

/// Read/write access handed to writable-atom write functions
/// (store.ts `writeAtomState`'s `readAtom` + `setter` pair).
pub struct WriteArgs<'a, V: AtomValue> {
    store: &'a Store<V>,
    self_id: AtomId,
}

impl<V: AtomValue> WriteArgs<'_, V> {
    /// Untracked read (store.ts passes raw `readAtom` as the write getter).
    pub fn get(&self, id: AtomId) -> V {
        read_atom(&self.store.inner, id)
    }

    /// store.ts `writeAtomState::setter`: writing the atom itself severs its
    /// dependencies and stores the value directly (selfSetDoesNotTriggerGetter
    /// contract); writing another atom recurses into its write path. Flushing
    /// is deferred to the outermost `set` — vanilla's `isSync` mechanics.
    pub fn set(&self, id: AtomId, value: V) {
        if id == self.self_id {
            let mut inner = self.store.inner.borrow_mut();
            inner.sever_dependencies(id);
            inner.set_atom_state(id, value);
        } else {
            self.store.write_atom_state(id, value);
        }
    }

    /// Self-set without knowing your own id (vanilla write fns close over
    /// their own atom entity; a Rust write fn is built before its id exists).
    pub fn set_self(&self, value: V) {
        self.set(self.self_id, value);
    }
}

/// Insertion-ordered pending map. store.ts `pendingMap.set(atom, prev)`
/// updates the value of an existing key while keeping its position; drain
/// order is insertion order.
#[derive(Default)]
pub(super) struct PendingQueue<V: AtomValue> {
    pub(super) order: Vec<AtomId>,
    pub(super) entries: HashMap<AtomId, Option<V>>,
}

impl<V: AtomValue> PendingQueue<V> {
    /// Vanilla quirk preserved: a repeated set OVERWRITES the recorded prev
    /// with the latest one (store.ts:217), so an a→b→a batch still publishes.
    fn upsert(&mut self, id: AtomId, prev: Option<V>) {
        if self.entries.insert(id, prev).is_none() {
            self.order.push(id);
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub(super) fn drain_ordered(&mut self) -> Vec<(AtomId, Option<V>)> {
        let order = std::mem::take(&mut self.order);
        let mut entries = std::mem::take(&mut self.entries);
        order
            .into_iter()
            .filter_map(|id| entries.remove(&id).map(|prev| (id, prev)))
            .collect()
    }
    pub(super) fn clear(&mut self) {
        self.order.clear();
        self.entries.clear();
    }
}

impl<V: AtomValue> Inner<V> {
    /// store.ts `setAtomState` minus the Promise branch: PartialEq
    /// short-circuit, store, generation bump, pending entry with prev.
    /// Returns true when the value actually changed.
    pub(super) fn set_atom_state(&mut self, id: AtomId, value: V) -> bool {
        let rec = self.record(id);
        let prev = rec.value.clone();
        // 008 清理：上游 `if let Some(prev_v) = &prev { if *prev_v == value {...} }`
        // 是 clippy::collapsible_if；折成一次比较，行为不变（prev 为 None 时两边
        // 都不会提前返回）。
        if prev.as_ref() == Some(&value) {
            return false;
        }
        let rec = self.record_mut(id);
        rec.value = Some(value);
        rec.generation += 1;
        self.write_seq += 1;
        self.pending.upsert(id, prev);
        true
    }
}

fn pending_value_changed<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, id: AtomId, prev: &Option<V>) -> bool {
    let inner_ref = inner.borrow();
    if !inner_ref.has(id) {
        return false;
    }
    let current = &inner_ref.record(id).value;
    match (current, prev) {
        (Some(c), Some(p)) => c != p,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// store.ts `dependenciesChange`, iterative pre-order DFS with change
/// pruning and the DV-4 settled-memo. Visits back-dependents in insertion
/// order; a dependent whose re-read leaves its value unchanged prunes its
/// subtree.
fn dependencies_change<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, root: AtomId) {
    let mut stack: Vec<AtomId> = Vec::new();
    {
        let inner_ref = inner.borrow();
        if !inner_ref.has(root) {
            return;
        }
        for dep in inner_ref.record(root).back_deps.iter_ordered() {
            stack.push(dep);
        }
        // Preserve store.ts forEach order under LIFO processing.
        stack.reverse();
    }
    while let Some(id) = stack.pop() {
        {
            let mut inner_mut = inner.borrow_mut();
            if !inner_mut.has(id) {
                continue;
            }
            inner_mut.flush_visit_count += 1;
            let seq = inner_mut.write_seq;
            if inner_mut.record(id).settled_at == seq {
                continue; // DV-4: already confirmed at this write sequence
            }
        }
        let before_gen = inner.borrow().record(id).generation;
        let _ = read_atom(inner, id);
        let (after_gen, children): (u64, Vec<AtomId>) = {
            let inner_ref = inner.borrow();
            let rec = inner_ref.record(id);
            (rec.generation, rec.back_deps.iter_ordered().collect())
        };
        if after_gen == before_gen {
            continue; // Object.is prune — subtree not visited
        }
        for child in children.into_iter().rev() {
            stack.push(child);
        }
    }
}

/// store.ts `flushPending`: drain in insertion order; for each drained entry
/// re-derive its dependents, then publish it if its post-flush state differs
/// from the recorded prev. Re-entrant sets from listeners land in `pending`
/// and are drained by the same while loop.
pub(super) fn flush_pending<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>) {
    loop {
        let drained = {
            let mut inner_mut = inner.borrow_mut();
            if inner_mut.pending.is_empty() {
                return;
            }
            inner_mut.pending.drain_ordered()
        };
        for (id, prev) in drained {
            dependencies_change(inner, id);
            if pending_value_changed(inner, id, &prev) {
                publish_atom(inner, id);
            }
        }
    }
}

/// Settle pending entries produced by a completed top-level read without
/// re-traversing the graph that the read just computed. Any listener writes
/// are handed back to the normal flush path so write propagation retains the
/// vanilla `flushPending` semantics.
pub(super) fn settle_pending_reads<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>) {
    let drained = {
        let mut inner_mut = inner.borrow_mut();
        if inner_mut.pending.is_empty() {
            return;
        }
        inner_mut.pending.drain_ordered()
    };
    for (id, prev) in drained {
        if pending_value_changed(inner, id, &prev) {
            publish_atom(inner, id);
        }
    }
    // A listener may synchronously write while the read results publish.
    // Those new entries were not part of the completed read and must perform
    // ordinary dependency propagation.
    flush_pending(inner);
}

impl<V: AtomValue> Store<V> {
    /// store.ts `setAtom`: write, then flush (synchronously — no async
    /// values in this port).
    pub fn set(&self, id: AtomId, value: V) {
        self.write_atom_state(id, value);
        if self.inner.borrow().batch_depth == 0 {
            flush_pending(&self.inner);
        }
    }

    /// store.ts `writeAtomState`: writable atoms delegate to their write fn
    /// (whose setter defers flushing — vanilla `isSync`); primitives assert
    /// and store directly.
    fn write_atom_state(&self, id: AtomId, value: V) {
        let write_fn = {
            let inner = self.inner.borrow();
            if !inner.has(id) {
                panic!("atom {:?} not found in store", id);
            }
            inner.record(id).write_fn.clone()
        };
        if let Some(write_fn) = write_fn {
            // RAII: a panicking write fn must not leave the id in `setting`
            // (poisoned guard — codex P1 review P2 #2, old SetGuard).
            let _guard = SettingGuard::enter(&self.inner, id);
            let args = WriteArgs {
                store: self,
                self_id: id,
            };
            write_fn(&args, value);
            return;
        }
        {
            let inner = self.inner.borrow();
            assert!(
                inner.record(id).read_fn.is_none(),
                "cannot set a read-only derived atom"
            );
        }
        self.inner.borrow_mut().set_atom_state(id, value);
    }

    /// Execute several writes with one flush at the end — the explicit form
    /// of a vanilla write-fn body. Nested batches flush once at depth 0.
    /// The depth guard survives a panicking body (codex P1 review P2 #3,
    /// old BatchGuard) — otherwise every later `set` would defer forever.
    pub fn batch(&self, f: impl FnOnce(&Self)) {
        {
            let _guard = BatchGuard::enter(&self.inner);
            f(self);
        }
        if self.inner.borrow().batch_depth == 0 {
            flush_pending(&self.inner);
        }
    }

    /// Public flush for engine call sites that used bare reads.
    pub fn flush(&self) {
        flush_pending(&self.inner);
    }

    /// Complete an engine-level read transaction. Bare `get` computes a
    /// coherent dependency graph before returning; this drains and publishes
    /// those read-originated pending entries without redundantly walking that
    /// graph once per computed atom.
    ///
    /// Callers must use this only at a top-level read boundary. Writes keep
    /// using `set` / `batch`, which invoke the full propagation flush.
    pub fn settle_pending_reads(&self) {
        settle_pending_reads(&self.inner);
    }
}
