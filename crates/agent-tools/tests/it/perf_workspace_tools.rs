//! 可撤回工作区工具的确定性资源预算测试。

use agent_tools::ToolExecutor;
use serde_json::{Value, json};
use crate::support::TestRoot;

const MAX_TEXT_BYTES: usize = 1_048_576;

fn inspect_absent(executor: &ToolExecutor) -> Value {
    let output = executor
        .execute("srv:fs/inspect", &json!({ "path": "generated.txt" }))
        .unwrap();
    serde_json::from_str(&output).unwrap()
}

#[test]
fn inspect_keeps_its_response_small_for_a_file_at_the_text_byte_limit() {
    let root = TestRoot::new("perf-workspace-tools");
    let executor = ToolExecutor::new(root.path()).unwrap();
    root.write("generated.txt", &"x".repeat(MAX_TEXT_BYTES));

    let inspected = executor
        .execute("srv:fs/inspect", &json!({ "path": "generated.txt" }))
        .unwrap();

    assert!(inspected.len() < 250, "inspect 不能回显 1 MiB 文件内容");
    assert_eq!(
        serde_json::from_str::<Value>(&inspected).unwrap()["exists"],
        json!(true)
    );
}

fn write_at_text_byte_limit(executor: &ToolExecutor) -> String {
    let inspected = inspect_absent(executor);
    assert_eq!(inspected["exists"], json!(false));

    executor
        .execute(
            "srv:fs/write_text",
            &json!({
                "path": "generated.txt",
                "content": "x".repeat(MAX_TEXT_BYTES),
                "expected_revision": inspected["revision"],
            }),
        )
        .unwrap()
}

#[test]
fn write_keeps_its_response_small_at_the_text_byte_limit() {
    let root = TestRoot::new("perf-workspace-write");
    let executor = ToolExecutor::new(root.path()).unwrap();
    let written = write_at_text_byte_limit(&executor);

    assert!(written.len() < 300, "工具结果不能回显 1 MiB 输入");
    assert_eq!(
        std::fs::metadata(root.path().join("generated.txt"))
            .unwrap()
            .len(),
        MAX_TEXT_BYTES as u64
    );
}

#[test]
fn revert_keeps_its_response_small_at_the_text_byte_limit() {
    let root = TestRoot::new("perf-workspace-revert");
    let executor = ToolExecutor::new(root.path()).unwrap();
    let written = write_at_text_byte_limit(&executor);
    let change_id = serde_json::from_str::<Value>(&written).unwrap()["change_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let reverted = executor
        .execute(
            "srv:workspace/revert_change",
            &json!({ "change_id": change_id }),
        )
        .unwrap();
    assert!(reverted.len() < 150);
    assert!(!root.path().join("generated.txt").exists());
}

#[test]
fn write_rejects_input_above_the_text_byte_budget_before_journaling() {
    let root = TestRoot::new("perf-workspace-over-budget");
    let executor = ToolExecutor::new(root.path()).unwrap();
    let inspected = inspect_absent(&executor);
    let error = executor
        .execute(
            "srv:fs/write_text",
            &json!({
                "path": "generated.txt",
                "content": "x".repeat(MAX_TEXT_BYTES + 1),
                "expected_revision": inspected["revision"],
            }),
        )
        .unwrap_err();

    assert_eq!(&*error.code, "file_too_large");
    assert!(!root.path().join(".agent/workspace-journal").exists());
}
