//! 一次 workspace mutation 的进程内与跨进程保护。
//!
//! 所有可变更工具先取得本 coordinator 的路径锁，再取得以 canonical workspace
//! 绝对路径为 key 的进程级锁，最后取得 journal 的 OS 锁。固定顺序避免同一进程
//! 的多个 executor 绕过彼此，也让不同进程不会在 revision 检查和实际写入之间
//! 产生 TOCTOU 窗口。

use crate::ToolError;
use crate::workspace::journal_record;
use crate::workspace::lock_set::{WorkspaceLockGuard, WorkspaceLockSet};
use crate::workspace::process_lock::{self, WorkspaceProcessLock};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static WORKSPACE_LOCKS: OnceLock<WorkspaceLockSet> = OnceLock::new();

/// 持有一次 mutation 所需的三层锁，离开作用域时按逆序释放。
pub(crate) struct WorkspaceMutationGuard<'a> {
    _local: WorkspaceLockGuard<'a>,
    _workspace: WorkspaceLockGuard<'static>,
    _process: WorkspaceProcessLock,
}

/// 获取所有变更目标的锁并验证 journal 没有未完成记录。
pub(crate) fn acquire<'a>(
    root: &Path,
    local_locks: &'a WorkspaceLockSet,
    targets: impl IntoIterator<Item = PathBuf>,
) -> Result<WorkspaceMutationGuard<'a>, ToolError> {
    let targets: Vec<_> = targets.into_iter().collect();
    let local = local_locks.acquire_many(targets.iter().cloned());
    let workspace = global_locks().acquire_many(targets.iter().map(|target| root.join(target)));
    let process = process_lock::acquire(root)?;
    journal_record::assert_healthy(root)?;
    Ok(WorkspaceMutationGuard {
        _local: local,
        _workspace: workspace,
        _process: process,
    })
}

fn global_locks() -> &'static WorkspaceLockSet {
    WORKSPACE_LOCKS.get_or_init(WorkspaceLockSet::default)
}

#[cfg(test)]
mod tests {
    use super::acquire;
    use crate::workspace::lock_set::WorkspaceLockSet;
    use std::path::PathBuf;
    use std::sync::mpsc;

    #[test]
    fn same_workspace_target_waits_across_two_coordinators() {
        let root =
            std::env::temp_dir().join(format!("agent-tools-mutation-guard-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let first = WorkspaceLockSet::default();
        let second = WorkspaceLockSet::default();
        let guard = acquire(&root, &first, [PathBuf::from("note.txt")]).unwrap();
        let (attempting_tx, attempting_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            let guard = acquire(&worker_root, &second, [PathBuf::from("note.txt")]).unwrap();
            acquired_tx.send(()).unwrap();
            drop(guard);
        });

        attempting_rx.recv().unwrap();
        assert!(acquired_rx.try_recv().is_err());
        drop(guard);
        acquired_rx.recv().unwrap();
        worker.join().unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
