//! `srv:fs/read` 正常路径（issue 013 验收 1）：全读 / 带 offset / 带
//! offset+limit，覆盖中文内容与多行文件。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn reads_whole_file() {
    let root = TestRoot::new("read-whole");
    let content = "line1\nline2\nline3";
    root.write("a.txt", content);
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:fs/read", &json!({ "path": "a.txt" }))
        .unwrap();
    assert_eq!(out, content);
}

#[test]
fn reads_with_offset() {
    let root = TestRoot::new("read-offset");
    root.write("b.txt", "one\ntwo\nthree\nfour");
    let exec = ToolExecutor::new(root.path()).unwrap();

    // offset 是 1-based 起始行：从第 3 行开始，即 "three"、"four"。
    let out = exec
        .execute("srv:fs/read", &json!({ "path": "b.txt", "offset": 3 }))
        .unwrap();
    assert_eq!(out, "three\nfour");
}

#[test]
fn reads_with_offset_and_limit() {
    let root = TestRoot::new("read-offset-limit");
    root.write("c.txt", "1\n2\n3\n4\n5\n6");
    let exec = ToolExecutor::new(root.path()).unwrap();

    // 从第 2 行起最多取 3 行：2、3、4。
    let out = exec
        .execute(
            "srv:fs/read",
            &json!({ "path": "c.txt", "offset": 2, "limit": 3 }),
        )
        .unwrap();
    assert_eq!(out, "2\n3\n4");
}

#[test]
fn reads_chinese_multiline_content_whole_and_ranged() {
    let root = TestRoot::new("read-chinese");
    let content = "第一行\n第二行：你好\n第三行 end";
    root.write("d.txt", content);
    let exec = ToolExecutor::new(root.path()).unwrap();

    let whole = exec
        .execute("srv:fs/read", &json!({ "path": "d.txt" }))
        .unwrap();
    assert_eq!(whole, content);

    let ranged = exec
        .execute(
            "srv:fs/read",
            &json!({ "path": "d.txt", "offset": 2, "limit": 1 }),
        )
        .unwrap();
    assert_eq!(ranged, "第二行：你好");
}

#[test]
fn reads_nested_relative_path() {
    let root = TestRoot::new("read-nested");
    root.write("sub/dir/e.txt", "nested content");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:fs/read", &json!({ "path": "sub/dir/e.txt" }))
        .unwrap();
    assert_eq!(out, "nested content");
}
