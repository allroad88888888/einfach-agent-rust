//! 删除、复制与移动的可撤回和并发行为测试。

use super::journal_record;
use super::revision::Revision;
use super::transaction::{WorkspaceChange, WorkspaceTransactionCoordinator};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-file-operations-{name}-{}-{sequence}",
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
fn delete_persists_preimage_and_revert_restores_the_file() {
    let root = TestRoot::new("delete-revert");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .delete_file("note.txt", &Revision::for_contents(b"before"))
        .unwrap();

    assert!(!file.exists());
    assert_eq!(change.after_revision(), &Revision::absent());
    assert_eq!(
        coordinator.revert(change.change_id()).unwrap(),
        Revision::for_contents(b"before")
    );
    assert_eq!(std::fs::read_to_string(file).unwrap(), "before");
}

#[test]
fn copy_replaces_only_destination_and_revert_restores_it() {
    let root = TestRoot::new("copy-revert");
    let source = root.path().join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, "source").unwrap();
    std::fs::write(&destination, "destination").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .copy_file(
            "source.txt",
            &Revision::for_contents(b"source"),
            "destination.txt",
            &Revision::for_contents(b"destination"),
        )
        .unwrap();

    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "source");
    assert_eq!(
        coordinator.revert(change.change_id()).unwrap(),
        Revision::for_contents(b"destination")
    );
    assert_eq!(std::fs::read_to_string(&source).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "destination");
}

#[test]
fn move_reverts_both_source_and_destination() {
    let root = TestRoot::new("move-revert");
    let source = root.path().join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, "source").unwrap();
    std::fs::write(&destination, "destination").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .move_file(
            "source.txt",
            &Revision::for_contents(b"source"),
            "destination.txt",
            &Revision::for_contents(b"destination"),
        )
        .unwrap();

    assert!(!source.exists());
    assert_eq!(std::fs::read_to_string(&destination).unwrap(), "source");
    assert_eq!(
        coordinator.revert(change.change_id()).unwrap(),
        Revision::for_contents(b"source")
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "destination");
}

#[test]
fn copy_rejects_a_stale_destination_revision_without_writing() {
    let root = TestRoot::new("copy-conflict");
    let source = root.path().join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, "source").unwrap();
    std::fs::write(&destination, "external").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let error = coordinator
        .copy_file(
            "source.txt",
            &Revision::for_contents(b"source"),
            "destination.txt",
            &Revision::for_contents(b"old destination"),
        )
        .unwrap_err();

    assert_eq!(&*error.code, "conflict");
    assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "external");
}

#[cfg(unix)]
#[test]
fn source_delete_failure_rolls_back_destination_and_finishes_journal() {
    use std::os::unix::fs::PermissionsExt;

    let root = TestRoot::new("move-rollback");
    let source_parent = root.path().join("locked");
    std::fs::create_dir(&source_parent).unwrap();
    let source = source_parent.join("source.txt");
    let destination = root.path().join("destination.txt");
    std::fs::write(&source, "source").unwrap();
    std::fs::write(&destination, "destination").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let permissions = std::fs::metadata(&source_parent).unwrap().permissions();
    let mut read_only = permissions.clone();
    read_only.set_mode(0o555);
    std::fs::set_permissions(&source_parent, read_only).unwrap();

    let error = coordinator
        .move_file(
            "locked/source.txt",
            &Revision::for_contents(b"source"),
            "destination.txt",
            &Revision::for_contents(b"destination"),
        )
        .unwrap_err();
    std::fs::set_permissions(&source_parent, permissions).unwrap();

    assert_eq!(&*error.code, "move_rolled_back");
    assert_eq!(std::fs::read_to_string(source).unwrap(), "source");
    assert_eq!(std::fs::read_to_string(destination).unwrap(), "destination");
    journal_record::assert_healthy(root.path()).unwrap();
}

#[test]
fn simultaneous_moves_of_one_source_have_one_winner_and_one_conflict() {
    let root = TestRoot::new("move-concurrency");
    std::fs::write(root.path().join("source.txt"), "source").unwrap();
    let first_coordinator = Arc::new(WorkspaceTransactionCoordinator::new(root.path()).unwrap());
    let second_coordinator = Arc::new(WorkspaceTransactionCoordinator::new(root.path()).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let (sender, receiver) = mpsc::channel();
    let first = spawn_move(&first_coordinator, &barrier, sender.clone(), "first.txt");
    let second = spawn_move(&second_coordinator, &barrier, sender, "second.txt");
    barrier.wait();
    let results = [receiver.recv().unwrap(), receiver.recv().unwrap()];
    first.join().unwrap();
    second.join().unwrap();

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| result
                .as_ref()
                .is_err_and(|error| &*error.code == "conflict"))
            .count(),
        1
    );
    assert!(!root.path().join("source.txt").exists());
    let moved = ["first.txt", "second.txt"]
        .into_iter()
        .filter(|name| {
            std::fs::read_to_string(root.path().join(name))
                .ok()
                .as_deref()
                == Some("source")
        })
        .count();
    assert_eq!(moved, 1);
}

fn spawn_move(
    coordinator: &Arc<WorkspaceTransactionCoordinator>,
    barrier: &Arc<Barrier>,
    sender: mpsc::Sender<Result<WorkspaceChange, crate::ToolError>>,
    destination: &'static str,
) -> std::thread::JoinHandle<()> {
    let coordinator = Arc::clone(coordinator);
    let barrier = Arc::clone(barrier);
    std::thread::spawn(move || {
        barrier.wait();
        sender
            .send(coordinator.move_file(
                "source.txt",
                &Revision::for_contents(b"source"),
                destination,
                &Revision::absent(),
            ))
            .unwrap();
    })
}
