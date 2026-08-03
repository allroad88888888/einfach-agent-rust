//! 跨 `ToolExecutor` 与跨进程的 workspace mutation 排他锁。
//!
//! 进程内的 [`super::lock_set::WorkspaceLockSet`] 只解决同一个 coordinator 内的
//! 并发；journal 是 workspace 共享状态，所以所有进程都还必须在进入事务前拿到
//! 这一把 OS 锁。锁粒度故意是整个 journal：它让 preimage、manifest、目标改写和
//! revert 成为一个跨进程串行序列，revision 冲突才不会被 TOCTOU 窗口绕过。

use crate::ToolError;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// 持有期间排他保护整个 workspace mutation journal 的 RAII 凭证。
#[derive(Debug)]
pub(crate) struct WorkspaceProcessLock {
    file: File,
}

/// 获取 workspace 的跨进程 mutation 锁。
pub(crate) fn acquire(root: &Path) -> Result<WorkspaceProcessLock, ToolError> {
    let journal = ensure_journal_dir(root)?;
    let lock_path = journal.join(".mutation.lock");
    let file = open_lock_file(&lock_path)?;
    lock_exclusively(&file)?;
    Ok(WorkspaceProcessLock { file })
}

fn ensure_journal_dir(root: &Path) -> Result<PathBuf, ToolError> {
    let agent = root.join(".agent");
    ensure_plain_dir(&agent)?;
    let journal = agent.join("workspace-journal");
    ensure_plain_dir(&journal)?;
    Ok(journal)
}

fn ensure_plain_dir(path: &Path) -> Result<(), ToolError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(tool_err(
                "journal_needs_repair",
                "workspace journal 包含 symlink",
            ));
        }
        Ok(metadata) if metadata.is_dir() => return Ok(()),
        Ok(_) => {
            return Err(tool_err(
                "journal_needs_repair",
                "workspace journal 路径不是目录",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    match fs::create_dir(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => ensure_plain_dir(path),
        Err(error) => Err(io_error(error)),
    }
}

#[cfg(unix)]
fn open_lock_file(path: &Path) -> Result<File, ToolError> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::ELOOP) {
                tool_err(
                    "journal_needs_repair",
                    "workspace journal lock 不能是 symlink",
                )
            } else {
                io_error(error)
            }
        })
}

#[cfg(not(unix))]
fn open_lock_file(path: &Path) -> Result<File, ToolError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(path)
        .map_err(io_error)
}

#[cfg(unix)]
fn lock_exclusively(file: &File) -> Result<(), ToolError> {
    use std::os::fd::AsRawFd;

    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result == 0 {
        Ok(())
    } else {
        Err(io_error(std::io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
fn lock_exclusively(_file: &File) -> Result<(), ToolError> {
    Ok(())
}

#[cfg(unix)]
impl Drop for WorkspaceProcessLock {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;

        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[cfg(not(unix))]
impl Drop for WorkspaceProcessLock {
    fn drop(&mut self) {}
}

fn io_error(error: std::io::Error) -> ToolError {
    tool_err(
        "journal_needs_repair",
        format!("workspace journal lock I/O 失败：{error}"),
    )
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::acquire;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> PathBuf {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "agent-tools-process-lock-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn creates_the_journal_lock_under_the_workspace_root() {
        let root = temp_root("create");
        let _lock = acquire(&root).unwrap();
        assert!(
            root.join(".agent/workspace-journal/.mutation.lock")
                .is_file()
        );
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_symlink_substituted_for_the_lock_file() {
        let root = temp_root("symlink");
        let journal = root.join(".agent/workspace-journal");
        std::fs::create_dir_all(&journal).unwrap();
        let outside = root.join("outside.lock");
        std::fs::write(&outside, "x").unwrap();
        std::os::unix::fs::symlink(&outside, journal.join(".mutation.lock")).unwrap();

        let error = acquire(&root).unwrap_err();
        assert_eq!(&*error.code, "journal_needs_repair");
    }
}
