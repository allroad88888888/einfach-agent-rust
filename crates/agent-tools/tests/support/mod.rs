//! 测试共用：造一个进程/线程独占的临时目录当 `ToolExecutor` 的 root，并在
//! `Drop` 时清理（issue 013 验收「测试结束清理临时目录，失败忽略」）。
//!
//! 每个集成测试二进制各自 `mod support;` 引入本文件一份拷贝——这是 Rust
//! `tests/<dir>/mod.rs` 的标准写法，不会被当成独立测试二进制。
//!
//! `dead_code` 允许：不是每个消费此模块的测试二进制都用到全部方法（比如
//! `mkdir` 只被需要子目录的测试用到），这是共享测试 helper 文件的正常形态。
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// 一个只属于当前测试的临时目录，`Drop` 时尽力删除（失败忽略）。
pub struct TestRoot(PathBuf);

impl TestRoot {
    /// `tag` 只用来让临时目录名可读，不参与任何断言。
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-tools-test-{}-{tag}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test root dir");
        TestRoot(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// 在 root 下写一个文件（`rel` 是相对路径），父目录自动创建。
    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let p = self.0.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir parent for test file");
        }
        std::fs::write(&p, contents).expect("write test file");
        p
    }

    /// 在 root 下建一个子目录（可多级）。
    pub fn mkdir(&self, rel: &str) {
        std::fs::create_dir_all(self.0.join(rel)).expect("mkdir test dir");
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
