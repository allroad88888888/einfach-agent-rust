//! 可撤回工作区工具适配层的行为测试。

use super::revision::Revision;
use super::tool_adapter::{inspect, revert_change, write_text};
use super::transaction::WorkspaceTransactionCoordinator;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestRoot(PathBuf);

impl TestRoot {
    fn new(name: &str) -> Self {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "agent-tools-workspace-adapter-{name}-{}-{sequence}",
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

fn output(result: String) -> Value {
    serde_json::from_str(&result).unwrap()
}

#[test]
fn inspect_write_and_revert_return_machine_usable_revisions() {
    let root = TestRoot::new("round-trip");
    std::fs::write(root.path().join("note.txt"), "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let inspected = output(inspect(&coordinator, &json!({ "path": "./note.txt" })).unwrap());
    assert_eq!(inspected["path"], "note.txt");
    assert_eq!(inspected["exists"], true);
    assert_eq!(
        inspected["revision"],
        Revision::for_contents(b"before").as_str()
    );

    let written = output(
        write_text(
            &coordinator,
            &json!({
                "path": "note.txt",
                "content": "after",
                "expected_revision": inspected["revision"],
            }),
        )
        .unwrap(),
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "after"
    );
    assert_eq!(written["before_revision"], inspected["revision"]);
    assert_eq!(
        written["after_revision"],
        Revision::for_contents(b"after").as_str()
    );

    let reverted =
        output(revert_change(&coordinator, &json!({ "change_id": written["change_id"] })).unwrap());
    assert_eq!(reverted["revision"], inspected["revision"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "before"
    );
}

#[test]
fn malformed_revision_and_unknown_input_fields_are_bad_input() {
    let root = TestRoot::new("bad-input");
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let invalid_revision = write_text(
        &coordinator,
        &json!({ "path": "note.txt", "content": "after", "expected_revision": "nope" }),
    )
    .unwrap_err();
    assert_eq!(&*invalid_revision.code, "bad_input");

    let unexpected_field =
        inspect(&coordinator, &json!({ "path": "note.txt", "extra": true })).unwrap_err();
    assert_eq!(&*unexpected_field.code, "bad_input");

    write_text(
        &coordinator,
        &json!({ "path": "empty.txt", "content": "", "expected_revision": "absent:v1" }),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("empty.txt")).unwrap(),
        ""
    );
}

#[test]
fn stale_revision_is_reported_without_overwriting_external_edit() {
    let root = TestRoot::new("conflict");
    let file = root.path().join("note.txt");
    std::fs::write(&file, "before").unwrap();
    let coordinator = WorkspaceTransactionCoordinator::new(root.path()).unwrap();

    let expected = Revision::for_contents(b"before");
    std::fs::write(&file, "external").unwrap();
    let error = write_text(
        &coordinator,
        &json!({
            "path": "note.txt",
            "content": "replacement",
            "expected_revision": expected.as_str(),
        }),
    )
    .unwrap_err();

    assert_eq!(&*error.code, "conflict");
    assert_eq!(std::fs::read_to_string(file).unwrap(), "external");
}
