//! 删除、复制与移动在文本大小边界的确定性资源测试。

use super::revision::Revision;
use super::text_file::MAX_TEXT_FILE_BYTES;
use super::transaction::WorkspaceTransactionCoordinator;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-file-operations-perf-{name}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn delete_accepts_and_reverts_a_one_mebibyte_text_file() {
    let root = TestRoot::new("delete-limit");
    let contents = text_at_limit(b'd');
    let file = root.path().join("delete.txt");
    std::fs::write(&file, &contents).unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .delete_file("delete.txt", &Revision::for_contents(&contents))
        .unwrap();

    assert!(!file.exists());
    coordinator.revert(change.change_id()).unwrap();
    assert_eq!(std::fs::read(file).unwrap(), contents);
}

#[test]
fn copy_accepts_two_one_mebibyte_text_files() {
    let root = TestRoot::new("copy-limit");
    let source_contents = text_at_limit(b's');
    let destination_contents = text_at_limit(b'd');
    let source = root.path().join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, &source_contents).unwrap();
    std::fs::write(&destination, &destination_contents).unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .copy_file(
            "source.txt",
            &Revision::for_contents(&source_contents),
            "destination.txt",
            &Revision::for_contents(&destination_contents),
        )
        .unwrap();

    assert_eq!(std::fs::read(&source).unwrap(), source_contents);
    assert_eq!(std::fs::read(&destination).unwrap(), source_contents);
    coordinator.revert(change.change_id()).unwrap();
    assert_eq!(std::fs::read(destination).unwrap(), destination_contents);
}

#[test]
fn move_accepts_two_one_mebibyte_text_files_and_reverts_both() {
    let root = TestRoot::new("move-limit");
    let source_contents = text_at_limit(b's');
    let destination_contents = text_at_limit(b'd');
    let source = root.path().join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, &source_contents).unwrap();
    std::fs::write(&destination, &destination_contents).unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .move_file(
            "source.txt",
            &Revision::for_contents(&source_contents),
            "destination.txt",
            &Revision::for_contents(&destination_contents),
        )
        .unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read(&destination).unwrap(), source_contents);
    coordinator.revert(change.change_id()).unwrap();
    assert_eq!(std::fs::read(source).unwrap(), source_contents);
    assert_eq!(std::fs::read(destination).unwrap(), destination_contents);
}

#[test]
fn delete_rejects_a_file_larger_than_the_bounded_text_budget() {
    let root = TestRoot::new("delete-over-limit");
    let contents = vec![b'x'; MAX_TEXT_FILE_BYTES + 1];
    std::fs::write(root.path().join("too-large.txt"), contents).unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let error = coordinator
        .delete_file("too-large.txt", &Revision::absent())
        .unwrap_err();

    assert_eq!(&*error.code, "file_too_large");
}

fn text_at_limit(byte: u8) -> Vec<u8> {
    vec![byte; MAX_TEXT_FILE_BYTES]
}
