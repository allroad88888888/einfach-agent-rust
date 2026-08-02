//! `srv:fs/list` 正常路径（issue 013 验收 1）：按名字典序排序、目录带尾 `/`、
//! `path` 省略时默认列 root（`"."`）。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn lists_entries_sorted_with_dir_trailing_slash() {
    let root = TestRoot::new("list-sorted");
    root.write("b.txt", "b");
    root.write("z.txt", "z");
    root.mkdir("a_dir");
    root.mkdir("m_dir");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:fs/list", &json!({ "path": "." }))
        .unwrap();
    let lines: Vec<&str> = out.lines().collect();

    assert_eq!(lines, vec!["a_dir/", "b.txt", "m_dir/", "z.txt"]);
}

#[test]
fn missing_path_defaults_to_root() {
    let root = TestRoot::new("list-default");
    root.write("only.txt", "x");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec.execute("srv:fs/list", &json!({})).unwrap();
    assert_eq!(out, "only.txt");
}

#[test]
fn lists_subdirectory() {
    let root = TestRoot::new("list-subdir");
    root.mkdir("sub");
    root.write("sub/inner.txt", "inner");
    root.write("top.txt", "top");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:fs/list", &json!({ "path": "sub" }))
        .unwrap();
    assert_eq!(out, "inner.txt");
}

#[test]
fn lists_empty_directory_as_empty_string() {
    let root = TestRoot::new("list-empty");
    root.mkdir("empty_dir");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:fs/list", &json!({ "path": "empty_dir" }))
        .unwrap();
    assert_eq!(out, "");
}
