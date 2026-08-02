//! Atom 图的数据结构：一个 atom 在依赖图里长什么样（`AtomRecord`）、它的反向依赖表
//! （`BackDeps`）、以及整个 store 的记录表（`Inner`）连同最基础的存取原语。
//! 计算/写入/订阅/销毁这些"行为"分别在别的子模块；这里只定义"状态长什么样"和
//! 少数不可再分的读取原语（`record` / `has` / `is_fresh` / `sever_dependencies`）。

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use crate::ids::AtomId;

use super::flush::PendingQueue;
use super::subscribe::{Listener, SubscriptionId};

/// Trait for value types that can be stored in atoms.
/// Defines the minimal interface required for the store to manage atom values.
pub trait AtomValue: Clone + PartialEq + std::fmt::Debug + 'static {
    /// Returns a null/default value (used as placeholder for pending atoms
    /// that exceed recursion budget). Corresponds to store.rs:551 in the
    /// upstream implementation.
    fn null() -> Self;
}

// 别名上不写约束：类型别名的泛型约束本就不被编译器强制（type_alias_bounds），
// 真正的约束在使用处的 impl<V: AtomValue> 上。
pub(super) type ReadFn<V> = Rc<dyn Fn(&super::read::ReadArgs<V>) -> V>;
pub(super) type WriteFn<V> = Rc<dyn Fn(&super::flush::WriteArgs<V>, V)>;

/// Insertion-ordered reverse-dependency set. JS `Set` iterates in insertion
/// order and store.ts's `dependenciesChange` visit order (hence recompute
/// counter determinism) depends on it; re-adding an existing member keeps its
/// original position, so commit-time dep diffs that leave an edge in place
/// preserve order exactly like `Set.add` of a present key.
#[derive(Default)]
pub(super) struct BackDeps {
    by_seq: BTreeMap<u64, AtomId>,
    seq_of: HashMap<AtomId, u64>,
    next_seq: u64,
}

impl BackDeps {
    pub(super) fn insert(&mut self, id: AtomId) {
        if self.seq_of.contains_key(&id) {
            return;
        }
        self.seq_of.insert(id, self.next_seq);
        self.by_seq.insert(self.next_seq, id);
        self.next_seq += 1;
    }
    pub(super) fn remove(&mut self, id: AtomId) {
        if let Some(seq) = self.seq_of.remove(&id) {
            self.by_seq.remove(&seq);
        }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.by_seq.is_empty()
    }
    pub(super) fn len(&self) -> usize {
        self.by_seq.len()
    }
    pub(super) fn iter_ordered(&self) -> impl Iterator<Item = AtomId> + '_ {
        self.by_seq.values().copied()
    }
}

pub(super) struct AtomRecord<V: AtomValue> {
    /// Current state. `None` = never read (store.ts: absent from
    /// `atomStateMap`).
    pub(super) value: Option<V>,
    /// DV-2 generation: bumped exactly when `value` is replaced.
    pub(super) generation: u64,
    /// Initial value for primitive atoms (`atom.init`).
    pub(super) init: Option<V>,
    pub(super) read_fn: Option<ReadFn<V>>,
    pub(super) write_fn: Option<WriteFn<V>>,
    /// Committed dependency snapshots in read order: `(dep, dep_generation)`.
    /// `None` = no `dependenciesMap` entry — store.ts:47-51: such an atom
    /// with a cached value is unconditionally fresh.
    pub(super) deps: Option<Vec<(AtomId, u64)>>,
    pub(super) back_deps: BackDeps,
    /// True while this atom's read is on the frame stack (cycle guard and
    /// store.ts:97-102 self-read detection).
    pub(super) computing: bool,
    /// Force-staleness flag (public `invalidate`, used by the engine's
    /// cycle-dissolve path from P4 on).
    pub(super) stale: bool,
    /// DV-4 settled-memo stamp.
    pub(super) settled_at: u64,
}

impl<V: AtomValue> AtomRecord<V> {
    pub(super) fn new_primitive(init: V) -> Self {
        AtomRecord {
            value: None,
            generation: 0,
            init: Some(init),
            read_fn: None,
            write_fn: None,
            deps: None,
            back_deps: BackDeps::default(),
            computing: false,
            stale: false,
            settled_at: 0,
        }
    }
    pub(super) fn new_derived(read_fn: ReadFn<V>, write_fn: Option<WriteFn<V>>) -> Self {
        AtomRecord {
            value: None,
            generation: 0,
            init: None,
            read_fn: None,
            write_fn,
            deps: None,
            back_deps: BackDeps::default(),
            computing: false,
            stale: false,
            settled_at: 0,
        }
        .with_read(read_fn)
    }
    fn with_read(mut self, read_fn: ReadFn<V>) -> Self {
        self.read_fn = Some(read_fn);
        self
    }
}

pub(super) struct Inner<V: AtomValue> {
    /// Monotonic ids, no slot reuse — matches the previous store (and keeps
    /// stale AtomIds in pending/subscriptions from aliasing a new atom).
    pub(super) records: HashMap<AtomId, AtomRecord<V>>,
    pub(super) next_id: u64,
    pub(super) pending: PendingQueue<V>,
    /// Write-side cycle guard (defensive divergence kept from the previous
    /// store: two writable atoms setting each other must panic, not abort
    /// the WASM instance via stack exhaustion).
    pub(super) setting: Vec<AtomId>,
    /// DV-4: bumped on every value-changing `set_atom_state`.
    pub(super) write_seq: u64,
    pub(super) subscriptions: HashMap<AtomId, Vec<(SubscriptionId, Listener)>>,
    pub(super) sub_index: HashMap<SubscriptionId, AtomId>,
    pub(super) next_sub_id: u64,
    pub(super) batch_depth: u32,
    /// Current native nesting of read-fn execution (DV-3 hybrid budget).
    pub(super) read_depth: usize,
    /// Cumulative COMPLETED derived read-fn runs (faulted partials excluded).
    pub(super) recompute_count: usize,
    /// Cumulative dependents visited by `dependencies_change` (the successor
    /// of the old engine's dirty-BFS visit counter).
    pub(super) flush_visit_count: usize,
}

impl<V: AtomValue> Inner<V> {
    pub(super) fn record(&self, id: AtomId) -> &AtomRecord<V> {
        self.records
            .get(&id)
            .unwrap_or_else(|| panic!("atom {:?} not found in store", id))
    }
    pub(super) fn record_mut(&mut self, id: AtomId) -> &mut AtomRecord<V> {
        self.records
            .get_mut(&id)
            .unwrap_or_else(|| panic!("atom {:?} not found in store", id))
    }
    pub(super) fn has(&self, id: AtomId) -> bool {
        self.records.contains_key(&id)
    }

    /// Shallow freshness — the literal translation of store.ts:47-62:
    /// present state + (no dep entry ⇒ fresh | every dep snapshot still
    /// current). Never recurses; consistency at rest is guaranteed by the
    /// eager flush, exactly as in vanilla.
    pub(super) fn is_fresh(&self, id: AtomId) -> bool {
        let rec = self.record(id);
        if rec.value.is_none() || rec.stale {
            return false;
        }
        match &rec.deps {
            None => true,
            Some(deps) => deps
                .iter()
                .all(|(dep, generation)|  self.has(*dep) && self.record(*dep).generation == *generation),
        }
    }

    /// store.ts `clearDependencies` — severs this atom's forward edges and
    /// the matching reverse edges. Used by the self-set branch and destroy;
    /// ordinary re-reads use the commit-time diff instead (same end state).
    pub(super) fn sever_dependencies(&mut self, id: AtomId) {
        let old = self.record_mut(id).deps.take();
        if let Some(old) = old {
            for (dep, _) in old {
                if self.has(dep) {
                    self.record_mut(dep).back_deps.remove(id);
                }
            }
        }
    }
}
