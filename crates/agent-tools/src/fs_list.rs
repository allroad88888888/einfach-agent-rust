//! `srv:fs/list`：列目录（不递归）。
//!
//! 输出每行一个条目，**按名字典序排序**，目录带尾 `/`——顺序必须确定，结果
//! 会进 prompt（issue 013）。目录不存在 → `code = "not_found"`；不是目录 →
//! `code = "bad_input"`。路径监狱见 `exec::resolve_in_root`。

use crate::ToolError;
use crate::exec::{Resolved, resolve_in_root, tool_err};
use serde_json::Value;
use std::path::Path;

pub(crate) fn list(root: &Path, input: &Value) -> Result<String, ToolError> {
    let path = extract_path(input)?;

    let canon = match resolve_in_root(root, &path)? {
        Resolved::Missing => return Err(tool_err("not_found", format!("目录不存在：{path}"))),
        Resolved::Existing(p) => p,
    };

    if !canon.is_dir() {
        return Err(tool_err("bad_input", format!("不是目录：{path}")));
    }

    let mut entries: Vec<(String, bool)> = std::fs::read_dir(&canon)
        .map_err(|e| tool_err("bad_input", format!("读取目录失败：{e}")))?
        .filter_map(Result::ok)
        .map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            (name, is_dir)
        })
        .collect();

    // 先按原始名字排序（字典序），再拼尾部 /——否则 "foo/" 和 "foo.txt" 会按
    // 拼接后的字符串比较，顺序会跟着后缀走而不是名字本身。
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let lines: Vec<String> = entries
        .into_iter()
        .map(|(name, is_dir)| if is_dir { format!("{name}/") } else { name })
        .collect();

    Ok(lines.join("\n"))
}

/// `path` 缺省 `"."`。缺失/`null` → 默认值；类型不对 → `bad_input`。
fn extract_path(input: &Value) -> Result<String, ToolError> {
    match input {
        Value::Null => Ok(".".to_string()),
        Value::Object(obj) => match obj.get("path") {
            None | Some(Value::Null) => Ok(".".to_string()),
            Some(Value::String(s)) => Ok(s.clone()),
            Some(_) => Err(tool_err("bad_input", "path 必须是字符串")),
        },
        _ => Err(tool_err("bad_input", "input 必须是对象")),
    }
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
            "agent-tools-fslist-{name}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_path_lists_root_sorted_dirs_have_trailing_slash() {
        let root = new_executor(temp_root("default")).unwrap();
        std::fs::write(root.join("b.txt"), "").unwrap();
        std::fs::create_dir_all(root.join("a_dir")).unwrap();
        std::fs::write(root.join("c.txt"), "").unwrap();

        let out = list(&root, &json!({})).unwrap();
        assert_eq!(out, "a_dir/\nb.txt\nc.txt");
    }

    #[test]
    fn missing_path_uses_dot_default() {
        let root = new_executor(temp_root("null-input")).unwrap();
        std::fs::write(root.join("only.txt"), "").unwrap();
        let out = list(&root, &Value::Null).unwrap();
        assert_eq!(out, "only.txt");
    }

    #[test]
    fn explicit_subdir_path() {
        let root = new_executor(temp_root("subdir")).unwrap();
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/inner.txt"), "").unwrap();
        let out = list(&root, &json!({"path": "sub"})).unwrap();
        assert_eq!(out, "inner.txt");
    }

    #[test]
    fn sorts_lexicographically_regardless_of_kind() {
        let root = new_executor(temp_root("sort")).unwrap();
        // "foo.txt" 应该排在 "foo_dir/" 之前（'.' < '_' 按字典序）。
        std::fs::write(root.join("foo.txt"), "").unwrap();
        std::fs::create_dir_all(root.join("foo_dir")).unwrap();
        let out = list(&root, &json!({})).unwrap();
        assert_eq!(out, "foo.txt\nfoo_dir/");
    }

    #[test]
    fn missing_dir_is_not_found() {
        let root = new_executor(temp_root("missingdir")).unwrap();
        let err = list(&root, &json!({"path": "nope"})).unwrap_err();
        assert_eq!(&*err.code, "not_found");
    }

    #[test]
    fn file_as_path_is_bad_input() {
        let root = new_executor(temp_root("fileaspath")).unwrap();
        std::fs::write(root.join("f.txt"), "").unwrap();
        let err = list(&root, &json!({"path": "f.txt"})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn non_object_non_null_input_is_bad_input() {
        let root = new_executor(temp_root("badinput")).unwrap();
        let err = list(&root, &json!([1, 2, 3])).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn wrong_type_path_is_bad_input() {
        let root = new_executor(temp_root("wrongtype")).unwrap();
        let err = list(&root, &json!({"path": 42})).unwrap_err();
        assert_eq!(&*err.code, "bad_input");
    }

    #[test]
    fn path_outside_root_is_rejected() {
        let base = temp_root("outside");
        std::fs::create_dir_all(base.join("root")).unwrap();
        let root = new_executor(base.join("root")).unwrap();
        std::fs::create_dir_all(base.join("secret_dir")).unwrap();
        let err = list(&root, &json!({"path": "../secret_dir"})).unwrap_err();
        assert_eq!(&*err.code, "outside_root");
    }
}
