//! 错误码矩阵（issue 013 验收 5）：未知工具 → `unknown_tool`；`path` 缺失/
//! 类型不对 → `bad_input`；不存在的文件 → `not_found`；offset 超总行数 →
//! `Ok("")`（非错误）。

use agent_tools::ToolExecutor;
use serde_json::json;
use crate::support::TestRoot;

#[test]
fn unknown_tool_name_is_rejected() {
    let root = TestRoot::new("err-unknown-tool");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/does-not-exist", &json!({ "path": "." }))
        .expect_err("未知工具名必须是 Err");
    assert_eq!(err.code.as_ref(), "unknown_tool");
}

#[test]
fn completely_unrelated_tool_name_is_unknown_tool() {
    let root = TestRoot::new("err-unrelated-tool");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("shell/exec", &json!({}))
        .expect_err("非本工具集的名字也必须是 Err");
    assert_eq!(err.code.as_ref(), "unknown_tool");
}

#[test]
fn fs_read_missing_path_is_bad_input() {
    let root = TestRoot::new("err-missing-path");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/read", &json!({}))
        .expect_err("path 缺失必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn fs_read_wrong_type_path_is_bad_input() {
    let root = TestRoot::new("err-wrong-type-path");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/read", &json!({ "path": 123 }))
        .expect_err("path 类型不对必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn fs_list_wrong_type_path_is_bad_input() {
    let root = TestRoot::new("err-list-wrong-type-path");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/list", &json!({ "path": true }))
        .expect_err("path 类型不对必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn fs_read_nonexistent_file_is_not_found() {
    let root = TestRoot::new("err-not-found");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/read", &json!({ "path": "nope.txt" }))
        .expect_err("不存在的文件必须是 Err");
    assert_eq!(err.code.as_ref(), "not_found");
}

#[test]
fn fs_read_path_that_is_a_directory_is_bad_input() {
    let root = TestRoot::new("err-not-a-file");
    root.mkdir("adir");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:fs/read", &json!({ "path": "adir" }))
        .expect_err("目录不是文件，必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn fs_read_offset_beyond_total_lines_is_empty_ok() {
    let root = TestRoot::new("err-offset-overflow");
    root.write("small.txt", "a\nb\nc");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute(
            "srv:fs/read",
            &json!({ "path": "small.txt", "offset": 100 }),
        )
        .expect("offset 超总行数不是错误，是空字符串");
    assert_eq!(out, "");
}
