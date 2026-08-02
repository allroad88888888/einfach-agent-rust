//! Panic-safe RAII guards for `Inner`'s transient flags. Every one of these
//! flags (`computing`, `read_depth`, `setting`, `batch_depth`) is a "we're
//! mid-operation" marker that a panicking read/write fn must not leave set —
//! a poisoned guard turns into a false circular-dependency panic (or a
//! permanently-deferred flush) on every later call, not just the one that
//! panicked. All four guards share the identical bump-on-enter /
//! unbump-on-drop shape; that's why they live together instead of next to
//! whichever algorithm happens to enter them first.

use std::cell::RefCell;
use std::rc::Rc;

use crate::ids::AtomId;

use super::records::{AtomValue, Inner};

/// RAII guard for the DV-3 nesting counter — a panicking read fn must not
/// leave `read_depth` elevated (that would silently push all future reads
/// onto the fault path).
pub(super) struct ReadDepthGuard<V: AtomValue> {
    inner: Rc<RefCell<Inner<V>>>,
}

impl<V: AtomValue> ReadDepthGuard<V> {
    pub(super) fn enter(inner: &Rc<RefCell<Inner<V>>>) -> Self {
        inner.borrow_mut().read_depth += 1;
        ReadDepthGuard {
            inner: inner.clone(),
        }
    }
}

impl<V: AtomValue> Drop for ReadDepthGuard<V> {
    fn drop(&mut self) {
        self.inner.borrow_mut().read_depth -= 1;
    }
}

/// RAII guard for the per-atom `computing` flag — a panicking read fn must
/// not leave the flag set (false circular-dependency panics on later reads;
/// codex P1 review P2 #1, the old store's RecomputeGuard equivalent).
pub(super) struct ComputingGuard<V: AtomValue> {
    inner: Rc<RefCell<Inner<V>>>,
    id: AtomId,
}

impl<V: AtomValue> ComputingGuard<V> {
    pub(super) fn enter(inner: &Rc<RefCell<Inner<V>>>, id: AtomId) -> Self {
        inner.borrow_mut().record_mut(id).computing = true;
        ComputingGuard {
            inner: inner.clone(),
            id,
        }
    }
}

impl<V: AtomValue> Drop for ComputingGuard<V> {
    fn drop(&mut self) {
        let mut inner = self.inner.borrow_mut();
        if inner.has(self.id) {
            inner.record_mut(self.id).computing = false;
        }
    }
}

/// RAII guard for the write-side cycle list (old store's SetGuard).
pub(super) struct SettingGuard<V: AtomValue> {
    inner: Rc<RefCell<Inner<V>>>,
    id: AtomId,
}

impl<V: AtomValue> SettingGuard<V> {
    pub(super) fn enter(inner: &Rc<RefCell<Inner<V>>>, id: AtomId) -> Self {
        {
            let mut inner_mut = inner.borrow_mut();
            if inner_mut.setting.contains(&id) {
                panic!(
                    "write-side circular dependency detected: atom {:?} is already being set",
                    id
                );
            }
            inner_mut.setting.push(id);
        }
        SettingGuard {
            inner: inner.clone(),
            id,
        }
    }
}

impl<V: AtomValue> Drop for SettingGuard<V> {
    fn drop(&mut self) {
        self.inner.borrow_mut().setting.retain(|s| *s != self.id);
    }
}

/// RAII guard for `batch_depth` (old store's BatchGuard).
pub(super) struct BatchGuard<V: AtomValue> {
    inner: Rc<RefCell<Inner<V>>>,
}

impl<V: AtomValue> BatchGuard<V> {
    pub(super) fn enter(inner: &Rc<RefCell<Inner<V>>>) -> Self {
        inner.borrow_mut().batch_depth += 1;
        BatchGuard {
            inner: inner.clone(),
        }
    }
}

impl<V: AtomValue> Drop for BatchGuard<V> {
    fn drop(&mut self) {
        self.inner.borrow_mut().batch_depth -= 1;
    }
}
