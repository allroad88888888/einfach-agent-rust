//! DV-3 迭代式求值状态机本体：`read_atom` 的显式帧栈，替代 `store.ts` 里递归的
//! `readAtom`（100k 深的公式链会打爆 1 MB 的 WASM 栈）。`commit_read` 是这个状态机
//! 提交一帧的唯一出口，`seed_primitive` 是它在栈底遇到 primitive 时的收口——三者共享
//! 同一份"帧栈 + 故障重跑"协议，拆开会让人来回跳文件对齐这一份协议。

use std::cell::RefCell;
use std::rc::Rc;

use crate::ids::AtomId;

use super::guards::ComputingGuard;
use super::handle::Store;
use super::read::{ReadArgs, Scratch};
use super::records::{AtomValue, Inner};

/// First read of a primitive: state ← init, pending entry seeded exactly like
/// vanilla's first `readAtom` (nextState = atom.init → setAtomState).
pub(super) fn seed_primitive<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, id: AtomId) -> V {
    let mut inner_mut = inner.borrow_mut();
    let init = inner_mut
        .record(id)
        .init
        .clone()
        .expect("primitive atom has an init value");
    inner_mut.set_atom_state(id, init.clone());
    init
}

/// store.ts `readAtom`, iterative (DV-3). Returns the atom's fresh value.
pub(super) fn read_atom<V: AtomValue>(inner: &Rc<RefCell<Inner<V>>>, root: AtomId) -> V {
    // Fast paths that need no frame.
    {
        let inner_ref = inner.borrow();
        if !inner_ref.has(root) {
            panic!("atom {:?} not found in store", root);
        }
        if inner_ref.is_fresh(root) {
            return inner_ref.record(root).value.clone().expect("fresh value");
        }
        if inner_ref.record(root).read_fn.is_none() {
            drop(inner_ref);
            return seed_primitive(inner, root);
        }
    }

    let mut stack: Vec<AtomId> = vec![root];
    while let Some(&id) = stack.last() {
        // A parent's retry re-validates; anything fresh just pops.
        let (fresh, is_primitive) = {
            let inner_ref = inner.borrow();
            (
                inner_ref.is_fresh(id),
                inner_ref.record(id).read_fn.is_none(),
            )
        };
        if fresh {
            stack.pop();
            let mut inner_mut = inner.borrow_mut();
            let seq = inner_mut.write_seq;
            inner_mut.record_mut(id).settled_at = seq;
            continue;
        }
        if is_primitive {
            seed_primitive(inner, id);
            stack.pop();
            continue;
        }

        let read_fn = {
            let inner_ref = inner.borrow();
            inner_ref
                .record(id)
                .read_fn
                .clone()
                .expect("derived atom has read fn")
        };
        let computing_guard = ComputingGuard::enter(inner, id);
        let scratch = RefCell::new(Scratch::new(id));
        // No borrows held across the read fn — its getter re-borrows per call
        // and may fault. The guard clears `computing` even if the fn panics.
        let next_value = {
            let args = ReadArgs {
                inner,
                scratch: &scratch,
            };
            read_fn(&args)
        };
        drop(computing_guard);
        let Scratch {
            deps: new_deps,
            needed,
            faulted,
            ..
        } = scratch.into_inner();

        let mut inner_mut = inner.borrow_mut();
        if faulted {
            // Discard scratch entirely; committed deps stay intact (the
            // store.ts:47-51 trap this protocol exists to avoid). Compute
            // the missing deps first, then retry this frame.
            drop(inner_mut);
            for dep in needed.into_iter().rev() {
                stack.push(dep);
            }
            continue;
        }
        commit_read(&mut inner_mut, id, new_deps, next_value);
        stack.pop();
    }

    inner
        .borrow()
        .record(root)
        .value
        .clone()
        .expect("read_atom leaves the root computed")
}

/// Commit of a completed read: replace the dep set (diff-based so unchanged
/// edges keep their position in the dep's insertion-ordered back-set — the
/// exact end state of vanilla's clearDependencies + re-add), store the value
/// via `set_atom_state`, stamp settled, count the completed run.
fn commit_read<V: AtomValue>(
    inner: &mut Inner<V>,
    id: AtomId,
    new_deps: Vec<(AtomId, u64)>,
    value: V,
) {
    let old_deps = inner.record_mut(id).deps.take().unwrap_or_default();
    // Set-backed diff keeps large fan-in commits linear (codex P1 review).
    let new_dep_set: std::collections::HashSet<AtomId> = new_deps.iter().map(|(d, _)| *d).collect();
    for (old_dep, _) in &old_deps {
        if !new_dep_set.contains(old_dep) && inner.has(*old_dep) {
            inner.record_mut(*old_dep).back_deps.remove(id);
        }
    }
    for (dep, _) in &new_deps {
        inner.record_mut(*dep).back_deps.insert(id);
    }
    // store.ts creates a dependenciesMap entry only when the getter ran at
    // least once; a zero-dep read stays entry-less and is cached forever.
    inner.record_mut(id).deps = if new_deps.is_empty() {
        None
    } else {
        Some(new_deps)
    };
    let rec = inner.record_mut(id);
    rec.stale = false;
    inner.set_atom_state(id, value);
    let seq = inner.write_seq;
    let rec = inner.record_mut(id);
    rec.settled_at = seq;
    inner.recompute_count += 1;
}

impl<V: AtomValue> Store<V> {
    /// Read the current value (store.ts `readAtom` via the public getter).
    /// Note the vanilla quirk kept on purpose: a bare read that (re)computes
    /// parks pending entries which publish on the NEXT flush; `sub` and every
    /// `set` flush, so this is unobservable through normal use.
    pub fn get(&self, id: AtomId) -> V {
        read_atom(&self.inner, id)
    }
}
