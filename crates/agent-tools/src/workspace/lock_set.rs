//! 工作区相对路径的进程内排他锁。
//!
//! 调用方以一次操作会影响到的全部目标调用 [`WorkspaceLockSet::acquire_many`]。
//! 它会原子地取得整组锁，因而不会出现“先拿到 A 再等待 B”的交叉等待。目录与
//! 其任意后代互斥，使未来的目录移动、删除也能复用相同的协调规则。

use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, MutexGuard};

/// 一个 workspace 内正在执行的可变更目标集合。
#[derive(Default)]
pub(crate) struct WorkspaceLockSet {
    active: Mutex<Vec<PathBuf>>,
    released: Condvar,
}

impl WorkspaceLockSet {
    /// 等待并原子取得 `targets` 的所有互斥锁。
    ///
    /// 目标必须已经由 workspace target parser 规范化为相对路径。空集合没有需要
    /// 锁定的路径，立即返回一个空 guard。
    pub(crate) fn acquire_many(
        &self,
        targets: impl IntoIterator<Item = PathBuf>,
    ) -> WorkspaceLockGuard<'_> {
        let targets = normalized_targets(targets);
        let mut active = lock_unpoisoned(&self.active);
        while conflicts_with_active(&active, &targets) {
            active = wait_unpoisoned(&self.released, active);
        }
        active.extend(targets.iter().cloned());
        WorkspaceLockGuard {
            lock_set: self,
            targets,
        }
    }

    /// 在不等待的前提下取得一组锁；存在冲突时返回 `None`。
    ///
    /// 该接口只服务确定性锁测试；正常变更操作应使用
    /// [`Self::acquire_many`] 等待自己的轮次。
    #[cfg(test)]
    pub(crate) fn try_acquire_many(
        &self,
        targets: impl IntoIterator<Item = PathBuf>,
    ) -> Option<WorkspaceLockGuard<'_>> {
        let targets = normalized_targets(targets);
        let mut active = lock_unpoisoned(&self.active);
        if conflicts_with_active(&active, &targets) {
            return None;
        }
        active.extend(targets.iter().cloned());
        Some(WorkspaceLockGuard {
            lock_set: self,
            targets,
        })
    }
}

/// 由 [`WorkspaceLockSet`] 返回的 RAII 锁凭证。
pub(crate) struct WorkspaceLockGuard<'a> {
    lock_set: &'a WorkspaceLockSet,
    targets: Vec<PathBuf>,
}

impl Drop for WorkspaceLockGuard<'_> {
    fn drop(&mut self) {
        if self.targets.is_empty() {
            return;
        }
        let mut active = lock_unpoisoned(&self.lock_set.active);
        for target in &self.targets {
            let position = active
                .iter()
                .position(|held| held == target)
                .expect("workspace lock guard must own every released target");
            active.swap_remove(position);
        }
        drop(active);
        self.lock_set.released.notify_all();
    }
}

fn normalized_targets(targets: impl IntoIterator<Item = PathBuf>) -> Vec<PathBuf> {
    let mut targets: Vec<_> = targets.into_iter().collect();
    targets.sort();
    targets.dedup();
    targets
}

fn conflicts_with_active(active: &[PathBuf], requested: &[PathBuf]) -> bool {
    requested.iter().any(|request| {
        active
            .iter()
            .any(|held| paths_conflict(request.as_path(), held.as_path()))
    })
}

fn paths_conflict(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait_unpoisoned<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::WorkspaceLockSet;
    use std::path::PathBuf;
    use std::sync::{Arc, Barrier, mpsc};

    fn target(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    #[test]
    fn same_path_waits_until_the_first_guard_is_released() {
        let locks = Arc::new(WorkspaceLockSet::default());
        let first = locks.acquire_many([target("src/lib.rs")]);
        let barrier = Arc::new(Barrier::new(2));
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker_locks = Arc::clone(&locks);
        let worker_barrier = Arc::clone(&barrier);

        let worker = std::thread::spawn(move || {
            worker_barrier.wait();
            attempting_tx.send(()).unwrap();
            let second = worker_locks.acquire_many([target("src/lib.rs")]);
            acquired_tx.send(()).unwrap();
            drop(second);
        });

        barrier.wait();
        attempting_rx.recv().unwrap();
        assert!(acquired_rx.try_recv().is_err());
        drop(first);
        acquired_rx.recv().unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn unrelated_paths_can_be_acquired_while_another_path_is_held() {
        let locks = WorkspaceLockSet::default();
        let _first = locks.acquire_many([target("src/lib.rs")]);

        assert!(locks.try_acquire_many([target("tests/lib.rs")]).is_some());
    }

    #[test]
    fn directory_and_descendant_paths_are_mutually_exclusive() {
        let locks = WorkspaceLockSet::default();
        let _directory = locks.acquire_many([target("src")]);

        assert!(
            locks
                .try_acquire_many([target("src/workspace/lock_set.rs")])
                .is_none()
        );
    }

    #[test]
    fn acquiring_multiple_targets_is_order_independent() {
        let locks = WorkspaceLockSet::default();
        let _guard = locks.acquire_many([target("b.txt"), target("a.txt"), target("a.txt")]);

        assert!(locks.try_acquire_many([target("c.txt")]).is_some());
        assert!(locks.try_acquire_many([target("a.txt")]).is_none());
    }
}
