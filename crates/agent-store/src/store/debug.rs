//! 面向诊断的只读探针（`#[doc(hidden)]`）：数原子/边/重算次数，不参与任何求值或传播，
//! 纯粹是"闭式统计量"，供测试和运维观测用。

use crate::ids::AtomId;

use super::handle::Store;
use super::records::AtomValue;

impl<V: AtomValue> Store<V> {
    #[doc(hidden)]
    pub fn debug_total_atom_count(&self) -> usize {
        self.inner.borrow().records.len()
    }

    #[doc(hidden)]
    pub fn debug_derived_atom_count(&self) -> usize {
        let inner = self.inner.borrow();
        inner
            .records
            .values()
            .filter(|r| r.read_fn.is_some())
            .count()
    }

    #[doc(hidden)]
    pub fn debug_dependent_count(&self, id: AtomId) -> usize {
        let inner = self.inner.borrow();
        if inner.has(id) {
            inner.record(id).back_deps.len()
        } else {
            0
        }
    }

    /// Whether `id` currently has a settled value whose dependency
    /// generations still match. Intended for diagnostics only; normal reads
    /// must use [`Store::get`], which re-derives stale atoms as needed.
    #[doc(hidden)]
    pub fn debug_atom_is_fresh(&self, id: AtomId) -> bool {
        let inner = self.inner.borrow();
        inner.has(id) && inner.is_fresh(id)
    }

    /// Total committed dependency edges (successor of the engine's
    /// dep-graph stats).
    #[doc(hidden)]
    pub fn debug_dependency_edge_count(&self) -> usize {
        let inner = self.inner.borrow();
        inner
            .records
            .values()
            .map(|r| r.deps.as_ref().map_or(0, |d| d.len()))
            .sum()
    }

    /// Completed derived read-fn runs since creation (never counts faulted
    /// partials — the DV-3 counter rule).
    #[doc(hidden)]
    pub fn debug_recompute_count(&self) -> usize {
        self.inner.borrow().recompute_count
    }

    /// Dependents visited by `dependencies_change` (successor of the old
    /// dirty-BFS visit counter).
    #[doc(hidden)]
    pub fn debug_flush_visit_count(&self) -> usize {
        self.inner.borrow().flush_visit_count
    }
}
