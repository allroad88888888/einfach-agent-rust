//! 可撤回文本写入的公开 ToolExecutor 接口验收。

mod support;

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use support::TestRoot;

fn call(executor: &ToolExecutor, tool: &str, input: Value) -> Value {
    let output = executor.execute(tool, &input).unwrap();
    serde_json::from_str(&output).unwrap()
}

#[test]
fn inspect_write_and_revert_form_an_explicit_revision_protocol() {
    let root = TestRoot::new("workspace-tools-round-trip");
    root.write("notes/todo.txt", "before");
    let executor = ToolExecutor::new(root.path()).unwrap();

    let inspected = call(
        &executor,
        "srv:fs/inspect",
        json!({ "path": "notes/todo.txt" }),
    );
    assert_eq!(inspected["path"], json!("notes/todo.txt"));
    assert_eq!(inspected["exists"], json!(true));

    let written = call(
        &executor,
        "srv:fs/write_text",
        json!({
            "path": "notes/todo.txt",
            "content": "after",
            "expected_revision": inspected["revision"],
        }),
    );
    assert_eq!(written["before_revision"], inspected["revision"]);
    assert_ne!(written["after_revision"], inspected["revision"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("notes/todo.txt")).unwrap(),
        "after"
    );

    let reverted = call(
        &executor,
        "srv:workspace/revert_change",
        json!({ "change_id": written["change_id"] }),
    );
    assert_eq!(reverted["revision"], inspected["revision"]);
    assert_eq!(
        std::fs::read_to_string(root.path().join("notes/todo.txt")).unwrap(),
        "before"
    );
}

#[test]
fn revision_conflict_never_overwrites_an_external_change() {
    let root = TestRoot::new("workspace-tools-conflict");
    root.write("note.txt", "before");
    let executor = ToolExecutor::new(root.path()).unwrap();
    let inspected = call(&executor, "srv:fs/inspect", json!({ "path": "note.txt" }));
    root.write("note.txt", "external");

    let error = executor
        .execute(
            "srv:fs/write_text",
            &json!({
                "path": "note.txt",
                "content": "replacement",
                "expected_revision": inspected["revision"],
            }),
        )
        .unwrap_err();

    assert_eq!(&*error.code, "conflict");
    assert_eq!(
        std::fs::read_to_string(root.path().join("note.txt")).unwrap(),
        "external"
    );
}
