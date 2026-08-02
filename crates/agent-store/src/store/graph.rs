//! 面向 engine adapter 的依赖图结构查询与 atom 收尾：反向可达性、直接依赖/被依赖快照、
//! 强制失效、销毁、整体重置。这些方法都不参与求值或传播——它们是 adapter 在求值状态机
//! 之外，自己动手维护派生结构（spill 目标、family 生命周期）时需要的只读探测 + 少量
//! 收尾写入，因此和"怎么算一个 atom 的值"分开放。

use std::collections::HashSet;

use crate::ids::AtomId;

use super::handle::Store;
use super::records::AtomValue;

impl<V: AtomValue> Store<V> {
    /// Force-mark a derived atom stale so its next read re-runs the read fn
    /// (engine cycle-dissolve). No-op on primitives and missing atoms.
    pub fn invalidate(&self, id: AtomId) {
        let mut inner = self.inner.borrow_mut();
        if inner.has(id) && inner.record(id).read_fn.is_some() {
            inner.record_mut(id).stale = true;
        }
    }

    /// Reverse-reachability over live back-dep edges: is any of `targets`
    /// reachable from `roots`? (Install-time cycle check for the engine.)
    pub fn reverse_reachable(&self, roots: &[AtomId], targets: &[AtomId]) -> bool {
        let inner = self.inner.borrow();
        let mut seen: Vec<AtomId> = Vec::new();
        let mut stack: Vec<AtomId> = roots.to_vec();
        while let Some(id) = stack.pop() {
            if targets.contains(&id) {
                return true;
            }
            if seen.contains(&id) || !inner.has(id) {
                continue;
            }
            seen.push(id);
            for dep in inner.record(id).back_deps.iter_ordered() {
                stack.push(dep);
            }
        }
        false
    }

    /// Enumerate reverse-reachable atoms over live back-dep edges.
    ///
    /// This exposes the same graph used by `dependencies_change` without
    /// adding a parallel dependency index. Callers that need to do work
    /// outside a pure atom recompute (for example sheet-level spill target
    /// maintenance) can map the returned atom ids back to their own families.
    pub fn reverse_dependents(&self, roots: &[AtomId]) -> Vec<AtomId> {
        let inner = self.inner.borrow();
        let mut seen: HashSet<AtomId> = HashSet::new();
        let mut out: Vec<AtomId> = Vec::new();
        let mut stack: Vec<AtomId> = roots.to_vec();
        while let Some(id) = stack.pop() {
            if !inner.has(id) {
                continue;
            }
            for dep in inner.record(id).back_deps.iter_ordered() {
                if seen.insert(dep) {
                    out.push(dep);
                    stack.push(dep);
                }
            }
        }
        out
    }

    pub fn has_atom(&self, id: AtomId) -> bool {
        self.inner.borrow().has(id)
    }

    /// Returns true if any other atom currently depends on `id`.
    pub fn has_dependents(&self, id: AtomId) -> bool {
        let inner = self.inner.borrow();
        inner.has(id) && !inner.record(id).back_deps.is_empty()
    }

    /// Snapshot the atoms that directly depend on `id`, in dependency
    /// registration order. Engine adapters use this when an indirection atom
    /// can be safely retargeted before releasing its old primitive backing.
    pub fn direct_dependents(&self, id: AtomId) -> Vec<AtomId> {
        let inner = self.inner.borrow();
        if !inner.has(id) {
            return Vec::new();
        }
        inner.record(id).back_deps.iter_ordered().collect()
    }

    /// Snapshot the atoms that `id` directly depends on, in read order.
    /// Engine adapters use this only for lifecycle cleanup after a derived
    /// atom has been detached; dependency propagation remains Store-owned.
    pub fn direct_dependencies(&self, id: AtomId) -> Vec<AtomId> {
        let inner = self.inner.borrow();
        if !inner.has(id) {
            return Vec::new();
        }
        inner
            .record(id)
            .deps
            .as_ref()
            .map(|deps| deps.iter().map(|(dep, _)| *dep).collect())
            .unwrap_or_default()
    }

    /// Destroy an atom and free all references to it. Panics if live
    /// downstream derived atoms remain (callers destroy dependents first).
    pub fn destroy_atom(&self, id: AtomId) {
        {
            let inner = self.inner.borrow();
            if !inner.has(id) {
                return;
            }
            if !inner.record(id).back_deps.is_empty() {
                panic!(
                    "cannot destroy atom {:?}: has {} live downstream derived atom(s)",
                    id,
                    inner.record(id).back_deps.len()
                );
            }
        }
        let mut inner = self.inner.borrow_mut();
        inner.sever_dependencies(id);
        if let Some(subs) = inner.subscriptions.remove(&id) {
            for (sub_id, _) in subs {
                inner.sub_index.remove(&sub_id);
            }
        }
        inner.records.remove(&id);
    }

    /// store.ts `clear()`: fresh maps AND a purged pendingMap (audit C-7 —
    /// old-world pending flushes must not leak into the new world).
    /// DIVERGENCE(store.ts): vanilla atoms are external objects that survive
    /// clear and re-materialize from `init` on next read; Rust atom
    /// definitions live in the store, so held AtomIds are dead after clear.
    /// The C-7 protective intent (no ghost flushes) is preserved and tested.
    pub fn clear(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.records.clear();
        inner.pending.clear();
        inner.setting.clear();
        inner.subscriptions.clear();
        inner.sub_index.clear();
        inner.write_seq = 0;
        inner.recompute_count = 0;
        inner.flush_visit_count = 0;
    }
}
