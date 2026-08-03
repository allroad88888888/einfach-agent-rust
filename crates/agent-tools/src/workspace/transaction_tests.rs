//! 单文件事务的确定性行为测试。

use super::journal_record::{self, OriginalContents};
use super::revision::Revision;
use super::target_path::parse_workspace_target;
use super::transaction::WorkspaceTransactionCoordinator;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-workspace-transaction-{name}-{}-{sequence}",
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
fn text_write_persists_a_committed_preimage_before_returning() {
    let root = TestRoot::new("write");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let change = coordinator
        .write_text("note.txt", &Revision::for_contents(b"before"), "after")
        .unwrap();

    assert_eq!(std::fs::read_to_string(&file).unwrap(), "after");
    assert_eq!(change.before_revision(), &Revision::for_contents(b"before"));
    assert_eq!(change.after_revision(), &Revision::for_contents(b"after"));
    let manifest = root
        .path()
        .join(".agent/workspace-journal/changes")
        .join(format!("{}.json", change.change_id()));
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    assert_eq!(value["phase"], "committed");
}

#[test]
fn stale_revision_returns_a_structured_conflict_without_writing() {
    let root = TestRoot::new("conflict");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let expected = Revision::for_contents(b"before");
    std::fs::write(&file, "changed elsewhere").unwrap();

    let error = coordinator
        .write_text("note.txt", &expected, "replacement")
        .unwrap_err();

    assert_eq!(&*error.code, "conflict");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "changed elsewhere");
}

#[test]
fn revert_restores_the_previous_contents_only_when_the_written_revision_survives() {
    let root = TestRoot::new("restore");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let change = coordinator
        .write_text("note.txt", &Revision::for_contents(b"before"), "after")
        .unwrap();

    let revision = coordinator.revert(change.change_id()).unwrap();

    assert_eq!(revision, Revision::for_contents(b"before"));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "before");
}

#[test]
fn revert_deletes_a_file_that_did_not_exist_before_the_change() {
    let root = TestRoot::new("delete-created");
    let file = root.path().join("new.txt");
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let change = coordinator
        .write_text("new.txt", &Revision::absent(), "created")
        .unwrap();

    let revision = coordinator.revert(change.change_id()).unwrap();

    assert_eq!(revision, Revision::absent());
    assert!(!file.exists());
}

#[test]
fn simultaneous_writes_with_the_same_revision_allow_exactly_one_winner() {
    let root = TestRoot::new("a-b");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = Arc::new(WorkspaceTransactionCoordinator::new(root.path()).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let (sender, receiver) = mpsc::channel();
    let expected = Revision::for_contents(b"before");

    let first = spawn_writer(
        &coordinator,
        &barrier,
        sender.clone(),
        expected.clone(),
        "first",
    );
    let second = spawn_writer(&coordinator, &barrier, sender, expected, "second");
    barrier.wait();
    let first_result = receiver.recv().unwrap();
    let second_result = receiver.recv().unwrap();
    first.join().unwrap();
    second.join().unwrap();

    let results = [first_result, second_result];
    let successes = results
        .iter()
        .filter(|result| result.as_ref().is_ok())
        .count();
    let conflicts = results
        .iter()
        .filter(|result| {
            result
                .as_ref()
                .is_err_and(|error| &*error.code == "conflict")
        })
        .count();
    assert_eq!((successes, conflicts), (1, 1));
    assert!(matches!(
        std::fs::read_to_string(file).unwrap().as_str(),
        "first" | "second"
    ));
}

#[test]
fn prepared_manifest_blocks_later_mutations_until_manual_repair() {
    let root = TestRoot::new("fail-closed");
    let target = parse_workspace_target(root.path(), "note.txt").unwrap();
    journal_record::prepare(
        root.path(),
        &target,
        &Revision::absent(),
        &OriginalContents::Absent,
        &Revision::for_contents(b"after"),
    )
    .unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let error = coordinator
        .write_text("note.txt", &Revision::absent(), "after")
        .unwrap_err();

    assert_eq!(&*error.code, "journal_needs_repair");
}

fn spawn_writer(
    coordinator: &Arc<WorkspaceTransactionCoordinator>,
    barrier: &Arc<Barrier>,
    sender: mpsc::Sender<Result<super::transaction::WorkspaceChange, crate::ToolError>>,
    expected: Revision,
    contents: &'static str,
) -> std::thread::JoinHandle<()> {
    let coordinator = Arc::clone(coordinator);
    let barrier = Arc::clone(barrier);
    std::thread::spawn(move || {
        barrier.wait();
        sender
            .send(coordinator.write_text("note.txt", &expected, contents))
            .unwrap();
    })
}
