use super::WorkspaceTransactionCoordinator;
use crate::workspace::patch_input::parse;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier, mpsc};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-apply-patch-{name}-{}-{sequence}",
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
fn applies_multiple_operations_as_one_revertible_change() {
    let root = TestRoot::new("revert");
    std::fs::write(root.path().join("delete.txt"), "delete").unwrap();
    std::fs::write(root.path().join("replace.txt"), "old old").unwrap();
    std::fs::write(root.path().join("overwrite.txt"), "previous").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let input = parse(&json!({
        "operations": [
            { "type": "add_file", "path": "added.txt", "content": "added" },
            { "type": "delete_file", "path": "delete.txt", "oldContent": "delete" },
            {
                "type": "replace", "path": "replace.txt", "oldText": "old", "newText": "new",
                "expectedReplacements": 2
            },
            {
                "type": "overwrite_file", "path": "overwrite.txt", "content": "next",
                "oldContent": "previous"
            }
        ]
    }))
    .unwrap();

    let result = coordinator.apply_patch(&input).unwrap();
    assert_eq!(result.changed_files.len(), 4);
    let change_id = result.change_id.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("added.txt")).unwrap(),
        "added"
    );
    assert!(!root.path().join("delete.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("replace.txt")).unwrap(),
        "new new"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("overwrite.txt")).unwrap(),
        "next"
    );

    coordinator.revert(&change_id).unwrap();
    assert!(!root.path().join("added.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("delete.txt")).unwrap(),
        "delete"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("replace.txt")).unwrap(),
        "old old"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("overwrite.txt")).unwrap(),
        "previous"
    );
}

#[test]
fn dry_run_and_conflicts_leave_every_file_unchanged() {
    let root = TestRoot::new("dry-run");
    std::fs::write(root.path().join("note.txt"), "old").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();
    let dry_run = parse(&json!({
        "dryRun": true,
        "operations": [{
            "type": "overwrite_file", "path": "note.txt", "content": "next", "oldContent": "old"
        }]
    }))
    .unwrap();
    let result = coordinator.apply_patch(&dry_run).unwrap();
    assert!(result.dry_run);
    assert!(result.change_id.is_none());
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "old"
    );

    let stale = parse(&json!({
        "operations": [{
            "type": "overwrite_file", "path": "note.txt", "content": "next", "oldContent": "before"
        }]
    }))
    .unwrap();
    let error = coordinator.apply_patch(&stale).unwrap_err();
    assert_eq!(&*error.code, "conflict");
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "old"
    );
}

#[test]
fn concurrent_agents_with_the_same_preimage_have_one_winner() {
    let root = TestRoot::new("concurrent");
    std::fs::write(root.path().join("note.txt"), "before").unwrap();
    let first = Arc::new(WorkspaceTransactionCoordinator::new(root.path()).unwrap());
    let second = Arc::new(WorkspaceTransactionCoordinator::new(root.path()).unwrap());
    let barrier = Arc::new(Barrier::new(3));
    let (sender, receiver) = mpsc::channel();
    let left = spawn_patch(&first, &barrier, sender.clone(), "first");
    let right = spawn_patch(&second, &barrier, sender, "second");
    barrier.wait();
    let results = [receiver.recv().unwrap(), receiver.recv().unwrap()];
    left.join().unwrap();
    right.join().unwrap();

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
    assert!(matches!(
        std::fs::read_to_string(root.path().join("note.txt"))
            .unwrap()
            .as_str(),
        "first" | "second"
    ));
}

fn spawn_patch(
    coordinator: &Arc<WorkspaceTransactionCoordinator>,
    barrier: &Arc<Barrier>,
    sender: mpsc::Sender<Result<(), crate::ToolError>>,
    replacement: &'static str,
) -> std::thread::JoinHandle<()> {
    let coordinator = Arc::clone(coordinator);
    let barrier = Arc::clone(barrier);
    std::thread::spawn(move || {
        let input = parse(&json!({
            "operations": [{
                "type": "overwrite_file", "path": "note.txt", "content": replacement,
                "oldContent": "before"
            }]
        }))
        .unwrap();
        barrier.wait();
        sender
            .send(coordinator.apply_patch(&input).map(|_| ()))
            .unwrap();
    })
}
