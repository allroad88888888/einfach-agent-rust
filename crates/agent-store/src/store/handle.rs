//! `Store` 句柄本身：一份指向 `Inner` 的引用计数句柄，克隆即共享（TS 版是一堆闭包共享
//! 同一批 map，`Rc<RefCell<Inner<V>>>` 是这个共享关系的 Rust 写法），以及怎么往里注册
//! 一个新的 primitive / derived / writable atom。别的子模块都只是往这个句柄上加
//! `impl<V: AtomValue> Store<V>` 方法块——句柄"是什么"和"怎么创建 atom 记录"是同一件事：
//! 分配一个新 id、塞进 `records` 表。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ids::AtomId;

use super::eval::read_atom;
use super::flush::{PendingQueue, WriteArgs};
use super::read::ReadArgs;
use super::records::{AtomRecord, AtomValue, Inner};

/// The central state container — a faithful port of the vanilla store.
/// All methods take `&self` (the TS store is a bundle of closures over
/// shared maps; `Rc<RefCell<Inner<V>>>` is the Rust spelling of that), so
/// listeners holding a clone can synchronously re-enter, exactly like JS.
pub struct Store<V: AtomValue> {
    pub(super) inner: Rc<RefCell<Inner<V>>>,
}

impl<V: AtomValue> Clone for Store<V> {
    fn clone(&self) -> Self {
        Store {
            inner: self.inner.clone(),
        }
    }
}

impl<V: AtomValue> Store<V> {
    pub fn new() -> Self {
        Store {
            inner: Rc::new(RefCell::new(Inner {
                records: HashMap::new(),
                next_id: 0,
                pending: PendingQueue {
                    order: Vec::new(),
                    entries: HashMap::new(),
                },
                setting: Vec::new(),
                write_seq: 0,
                subscriptions: HashMap::new(),
                sub_index: HashMap::new(),
                next_sub_id: 0,
                batch_depth: 0,
                read_depth: 0,
                recompute_count: 0,
                flush_visit_count: 0,
            })),
        }
    }

    fn alloc(&self, record: AtomRecord<V>) -> AtomId {
        let mut inner = self.inner.borrow_mut();
        let id = AtomId::from_raw(inner.next_id);
        inner.next_id += 1;
        inner.records.insert(id, record);
        id
    }

    /// Create a primitive atom with an initial value (`atom(init)`).
    pub fn create_atom(&self, init: V) -> AtomId {
        self.alloc(AtomRecord::new_primitive(init))
    }

    /// Create a read-only derived atom (`atom(read)`).
    ///
    /// Compatibility: unlike vanilla (lazy until first read), this
    /// legacy-signature API computes eagerly at creation because the
    /// current sheet engine's spill targets rely on the back-dep edge
    /// existing immediately (`has_dependents` guards anchor destruction).
    /// New code should use the vanilla-faithful `create_derived_ctx`.
    pub fn create_derived(&self, read_fn: impl Fn(&dyn Fn(AtomId) -> V) -> V + 'static) -> AtomId {
        let id = self.create_derived_ctx(move |args| read_fn(&|id| args.get(id)));
        let _ = read_atom(&self.inner, id);
        id
    }

    /// Full-context variant exposing the untracked `peek` (noWatch getter).
    /// LAZY like vanilla: nothing computes until the first read.
    pub fn create_derived_ctx(&self, read_fn: impl Fn(&ReadArgs<V>) -> V + 'static) -> AtomId {
        self.alloc(AtomRecord::new_derived(Rc::new(read_fn), None))
    }

    /// Create a writable derived atom (`atom(read, write)`).
    pub fn create_writable(
        &self,
        read_fn: impl Fn(&ReadArgs<V>) -> V + 'static,
        write_fn: impl Fn(&WriteArgs<V>, V) + 'static,
    ) -> AtomId {
        self.alloc(AtomRecord::new_derived(
            Rc::new(read_fn),
            Some(Rc::new(write_fn)),
        ))
    }
}

impl<V: AtomValue> Default for Store<V> {
    fn default() -> Self {
        Self::new()
    }
}
