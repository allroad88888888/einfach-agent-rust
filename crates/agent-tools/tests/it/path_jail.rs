//! 路径监狱（issue 013 验收 2，原文「不能读仓库外的文件」）：`../` 逃逸、
//! 绝对路径、（unix）指向 root 外的 symlink，全部必须 `Err` 且
//! `code == "outside_root"`，绝不返回内容。
//!
//! `../` 逃逸用不存在的目标：越界检查必须先于存在性检查生效，否则会把
//! outside_root 误判成 not_found，等于把「root 外是否存在该文件」泄露给调用方。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

fn assert_outside_root(exec: &ToolExecutor, tool: &str, input: serde_json::Value) {
    let err = exec
        .execute(tool, &input)
        .expect_err("越界访问必须返回 Err，绝不能返回内容");
    assert_eq!(err.code.as_ref(), "outside_root");
}

#[test]
fn fs_read_rejects_dotdot_escape() {
    let root = TestRoot::new("jail-read-dotdot");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/read", json!({ "path": "../x" }));
}

#[test]
fn fs_read_rejects_nested_dotdot_escape() {
    let root = TestRoot::new("jail-read-nested-dotdot");
    root.mkdir("a");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/read", json!({ "path": "a/../../x" }));
}

#[test]
fn fs_read_rejects_absolute_path_escape() {
    let root = TestRoot::new("jail-read-absolute");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/read", json!({ "path": "/etc/passwd" }));
}

#[test]
fn fs_list_rejects_dotdot_escape() {
    let root = TestRoot::new("jail-list-dotdot");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/list", json!({ "path": "../" }));
}

#[test]
fn fs_list_rejects_nested_dotdot_escape() {
    let root = TestRoot::new("jail-list-nested-dotdot");
    root.mkdir("a");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/list", json!({ "path": "a/../../etc" }));
}

#[test]
fn fs_list_rejects_absolute_path_escape() {
    let root = TestRoot::new("jail-list-absolute");
    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/list", json!({ "path": "/etc" }));
}

#[cfg(unix)]
#[test]
fn fs_read_rejects_symlink_escaping_root() {
    let root = TestRoot::new("jail-read-symlink");
    let outside = TestRoot::new("jail-read-symlink-outside");
    outside.write("secret.txt", "leak me not");

    let link = root.path().join("escape");
    std::os::unix::fs::symlink(outside.path().join("secret.txt"), &link)
        .expect("create symlink for test");

    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/read", json!({ "path": "escape" }));
}

#[cfg(unix)]
#[test]
fn fs_list_rejects_symlink_dir_escaping_root() {
    let root = TestRoot::new("jail-list-symlink");
    let outside = TestRoot::new("jail-list-symlink-outside");
    outside.write("inner.txt", "leak me not either");

    let link = root.path().join("escape_dir");
    std::os::unix::fs::symlink(outside.path(), &link).expect("create symlink dir for test");

    let exec = ToolExecutor::new(root.path()).unwrap();
    assert_outside_root(&exec, "srv:fs/list", json!({ "path": "escape_dir" }));
}
