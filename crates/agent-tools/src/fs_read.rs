//! `srv:fs/read`：读文件，可选行范围。
//!
//! 输出是选中行的原文（不带行号），行以 `\n` 连接。文件不存在 →
//! `code = "not_found"`；不是文件 → `code = "bad_input"`；`offset` 超过总行数
//! → 空字符串（不是错误）。路径监狱见 `exec::resolve_in_root`。

use crate::ToolError;
use crate::exec::{Resolved, resolve_in_root, tool_err};
use serde_json::{Map, Value};
use std::path::Path;

pub(crate) fn read(root: &Path, input: &Value) -> Result<String, ToolError> {
    let obj = input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;

    let path = obj
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| tool_err("bad_input", "path 是必填字符串"))?;

    let offset = parse_optional_ge1(obj, "offset")?;
    let limit = parse_optional_ge1(obj, "limit")?;

    let canon = match resolve_in_root(root, path)? {
        Resolved::Missing => return Err(tool_err("not_found", format!("文件不存在：{path}"))),
        Resolved::Existing(p) => p,
    };

    if !canon.is_file() {
        return Err(tool_err("bad_input", format!("不是文件：{path}")));
    }

    let content = std::fs::read_to_string(&canon)
        .map_err(|e| tool_err("bad_input", format!("读取失败：{e}")))?;

    Ok(select_lines(&content, offset, limit))
}

/// 解析一个可选的、值须 ≥1 的整数字段。缺失/`null` → `None`；类型不对或
/// 值 <1 → `bad_input`。
fn parse_optional_ge1(obj: &Map<String, Value>, key: &str) -> Result<Option<u64>, ToolError> {
    match obj.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|&n| n >= 1)
            .map(Some)
            .ok_or_else(|| tool_err("bad_input", format!("{key} 必须是 ≥1 的整数"))),
    }
}

/// 按 1-based `offset`（默认 1）与 `limit`（默认到文件末尾）选中行，
/// 用 `\n` 重新连接。`offset` 超过总行数返回空字符串。
pub(crate) fn select_lines(content: &str, offset: Option<u64>, limit: Option<u64>) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len() as u64;

    let start_1based = offset.unwrap_or(1);
    if start_1based > total {
        return String::new();
    }
    let start = (start_1based - 1) as usize;
    let end = match limit {
        Some(l) => start.saturating_add(l as usize).min(lines.len()),
        None => lines.len(),
    };
    lines[start..end].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::new_executor;
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(name: &str) -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "agent-tools-fsread-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn reads_whole_file_without_line_numbers() {
        let root = new_executor(temp_root("whole")).unwrap();
        std::fs::write(root.join("a.txt"), "one\ntwo\nthree").unwrap();
        let out = read(&root, &json!({"path": "a.txt"})).unwrap();
        assert_eq!(out, "one\ntwo\nthree");
    }

    #[test]
    fn offset_and_limit_select_a_window() {
        let root = new_executor(temp_root("window")).unwrap();
        std::fs::write(root.join("a.txt"), "1\n2\n3\n4\n5").unwrap();
        let out = read(&root, &json!({"path": "a.txt", "offset": 2, "limit": 2})).unwrap();
        assert_eq!(out, "2\n3");
    }

    #[test]
    fn offset_beyond_end_returns_empty_string_not_error() {
        let root = new_executor(temp_root("beyond")).unwrap();
        std::fs::write(root.join("a.txt"), "1\n2\n3").unwrap();
        let out = read(&root, &json!({"path": "a.txt", "offset": 100})).unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn missing_file_is_not_found() {
        let root = new_executor(temp_root("missing")).unwrap();
        let err = read(&root, &json!({"path": "nope.txt"})).unwrap_err();
        assert_eq!(&*err.code, "not_found");
    }

    #[test]
    fn directory_as_path_is_bad_input() {
        let root = new_executor(temp_root("isdir")).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let err = read(&root, &json!({"path": "sub"})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn missing_path_field_is_bad_input() {
        let root = new_executor(temp_root("nopath")).unwrap();
        let err = read(&root, &json!({})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn non_object_input_is_bad_input() {
        let root = new_executor(temp_root("nonobj")).unwrap();
        let err = read(&root, &json!("not-an-object")).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn offset_zero_is_bad_input() {
        let root = new_executor(temp_root("zero")).unwrap();
        std::fs::write(root.join("a.txt"), "1\n2").unwrap();
        let err = read(&root, &json!({"path": "a.txt", "offset": 0})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn wrong_type_offset_is_bad_input() {
        let root = new_executor(temp_root("wrongtype")).unwrap();
        std::fs::write(root.join("a.txt"), "1\n2").unwrap();
        let err = read(&root, &json!({"path": "a.txt", "offset": "two"})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn path_outside_root_is_rejected() {
        let base = temp_root("outside");
        std::fs::create_dir_all(base.join("root")).unwrap();
        let root = new_executor(base.join("root")).unwrap();
        std::fs::write(base.join("secret.txt"), "top secret").unwrap();
        let err = read(&root, &json!({"path": "../secret.txt"})).unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }

    #[test]
    fn absolute_path_is_rejected() {
        let root = new_executor(temp_root("absolute")).unwrap();
        let err = read(&root, &json!({"path": "/etc/passwd"})).unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }
}
