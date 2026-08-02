//! atom 变更的订阅登记与分发：谁在听一个 atom（`sub`/`unsub`/`has_subscribers`），
//! 以及变更发生时怎么通知到他们（`publish_atom` 快照监听器列表后释放所有借用再派发，
//! 因为监听器可能同步重入 store）。登记和分发是同一个"订阅"概念的两面，拆开只会让人
//! 在两个文件里对同一份 `subscriptions` 表做心智同步。

use std::cell::RefCell;
use std::rc::Rc;

use crate::ids::AtomId;

use super::eval::read_atom;
use super::flush::flush_pending;
use super::handle::Store;
use super::records::{AtomValue, Inner};

/// Trait-based subscription target (unchanged from the previous store — the
/// WASM crate's `JsCallbackListener` and `Fn()` closures both satisfy it).
pub trait CellListener: 'static {
    fn on_change(&self);
}

impl<F: Fn() + 'static> CellListener for F {
    fn on_change(&self) {
        self()
    }
}

pub(super) type Listener = Rc<dyn CellListener>;

/// Unique identifier for a subscription.
#[derive(Clone, Copy, Hash, Eq, PartialEq, Debug)]
pub struct SubscriptionId(u64);

fn listeners_snapshot<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, id: AtomId) -> Vec<Listener> {
    inner
        .borrow()
        .subscriptions
        .get(&id)
        .map(|subs| subs.iter().map(|(_, l)| l.clone()).collect())
        .unwrap_or_default()
}

/// store.ts `publishAtom` — snapshot the listener list, release all borrows,
/// dispatch. Listeners may synchronously re-enter the store (`set`, `sub`);
/// re-entrant sets land in `pending` and drain in the enclosing flush loop.
pub(super) fn publish_atom<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, id: AtomId) {
    for listener in listeners_snapshot(inner, id) {
        listener.on_change();
    }
}

impl<V: AtomValue> Store<V> {
    /// store.ts `subscribeAtom` via the public `sub` name (vanilla's store
    /// object exposes it as `sub` too).
    pub fn sub(&self, id: AtomId, listener: impl CellListener) -> SubscriptionId {
        self.subscribe_atom(id, Rc::new(listener))
    }

    /// Boxed variant for adapter layers (kept from the previous store).
    pub fn sub_boxed(&self, id: AtomId, listener: Box<dyn CellListener>) -> SubscriptionId {
        self.subscribe_atom(id, Rc::from(listener))
    }

    /// store.ts `subscribeAtom`: mount by reading, flush, then register.
    fn subscribe_atom(&self, id: AtomId, listener: Listener) -> SubscriptionId {
        let _ = read_atom(&self.inner, id);
        flush_pending(&self.inner);
        let mut inner = self.inner.borrow_mut();
        let sub_id = SubscriptionId(inner.next_sub_id);
        inner.next_sub_id += 1;
        inner
            .subscriptions
            .entry(id)
            .or_default()
            .push((sub_id, listener));
        inner.sub_index.insert(sub_id, id);
        sub_id
    }

    /// Remove a subscription. O(1) via the reverse index.
    pub fn unsub(&self, sub_id: SubscriptionId) {
        let mut inner = self.inner.borrow_mut();
        // 008 清理：上游 `if let Some(atom_id) = ... { if let Some(subs) = ... {...} }`
        // 是 clippy::collapsible_if；改成 let-else 提前返回，行为不变（任一 lookup
        // 落空都是无操作）。
        let Some(atom_id) = inner.sub_index.remove(&sub_id) else {
            return;
        };
        let Some(subs) = inner.subscriptions.get_mut(&atom_id) else {
            return;
        };
        subs.retain(|(id, _)| *id != sub_id);
        if subs.is_empty() {
            inner.subscriptions.remove(&atom_id);
        }
    }

    /// Returns true if the atom has live subscribers (AtomFamily eviction
    /// safety check).
    pub fn has_subscribers(&self, id: AtomId) -> bool {
        let inner = self.inner.borrow();
        inner.subscriptions.get(&id).is_some_and(|s| !s.is_empty())
    }
}
