//! 标准名称的工作区工具必须走同一套 revision/journal 实现，不能只在 schema 层存在。

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use crate::support::TestRoot;

fn call(executor: &ToolExecutor, tool: &str, input: Value) -> Value {
    let output = executor.execute(tool, &input).unwrap();
    serde_json::from_str(&output).unwrap()
}

fn read_file(executor: &ToolExecutor, path: &str) -> Value {
    call(executor, "read_file", json!({ "path": path }))
}

fn revert(executor: &ToolExecutor, change: &Value) {
    call(
        executor,
        "revert_workspace_change",
        json!({ "change_id": change["change_id"] }),
    );
}

#[test]
fn standard_read_file_returns_an_absent_revision_for_safe_creation() {
    let root = TestRoot::new("standard-workspace-create");
    let executor = ToolExecutor::new(root.path()).unwrap();

    let missing = read_file(&executor, "new.txt");
    assert_eq!(missing["path"], "new.txt");
    assert_eq!(missing["exists"], false);
    assert_eq!(missing["content"], "");
    assert_eq!(missing["revision"], "absent:v1");

    let created = call(
        &executor,
        "write_file",
        json!({
            "path": "new.txt",
            "content": "created",
            "expected_revision": missing["revision"],
        }),
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("new.txt")).unwrap(),
        "created"
    );
    revert(&executor, &created);
    assert!(!root.path().join("new.txt").exists());
}

#[test]
fn standard_mutation_names_use_revisions_and_one_revert_receipt() {
    let root = TestRoot::new("standard-workspace-tools");
    root.write("source.txt", "source");
    root.write("destination.txt", "destination");
    root.write("delete.txt", "delete");
    root.write("write.txt", "before");
    root.write("patch.txt", "old");
    let executor = ToolExecutor::new(root.path()).unwrap();

    let written_before = read_file(&executor, "write.txt");
    assert_eq!(written_before["content"], "before");
    let written = call(
        &executor,
        "write_file",
        json!({
            "path": "write.txt",
            "content": "after",
            "expected_revision": written_before["revision"],
        }),
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("write.txt")).unwrap(),
        "after"
    );
    revert(&executor, &written);

    let deleted_before = read_file(&executor, "delete.txt");
    let deleted = call(
        &executor,
        "delete_path",
        json!({ "path": "delete.txt", "expected_revision": deleted_before["revision"] }),
    );
    assert!(!root.path().join("delete.txt").exists());
    revert(&executor, &deleted);

    let source_before = read_file(&executor, "source.txt");
    let destination_before = read_file(&executor, "destination.txt");
    let copied = call(
        &executor,
        "copy_path",
        json!({
            "source": "source.txt",
            "destination": "destination.txt",
            "expected_source_revision": source_before["revision"],
            "expected_destination_revision": destination_before["revision"],
        }),
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("destination.txt")).unwrap(),
        "source"
    );
    revert(&executor, &copied);

    let source_before = read_file(&executor, "source.txt");
    let destination_before = read_file(&executor, "destination.txt");
    let moved = call(
        &executor,
        "move_path",
        json!({
            "source": "source.txt",
            "destination": "destination.txt",
            "expected_source_revision": source_before["revision"],
            "expected_destination_revision": destination_before["revision"],
        }),
    );
    assert!(!root.path().join("source.txt").exists());
    assert_eq!(
        std::fs::read_to_string(root.path().join("destination.txt")).unwrap(),
        "source"
    );
    revert(&executor, &moved);

    let patched = call(
        &executor,
        "apply_patch",
        json!({
            "operations": [{
                "type": "replace",
                "path": "patch.txt",
                "oldText": "old",
                "newText": "new",
            }]
        }),
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("patch.txt")).unwrap(),
        "new"
    );
    revert(&executor, &patched);

    assert_eq!(
        std::fs::read_to_string(root.path().join("write.txt")).unwrap(),
        "before"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("delete.txt")).unwrap(),
        "delete"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("source.txt")).unwrap(),
        "source"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("destination.txt")).unwrap(),
        "destination"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("patch.txt")).unwrap(),
        "old"
    );
}
