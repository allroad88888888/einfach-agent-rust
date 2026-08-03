//! `srv:fs/search_files`：按文件名查找 workspace 内的常规文件。
//!
//! `query` 不含通配符时是文件名子串；含 `*` 或 `?` 时是完整文件名 glob。
//! 遍历、路径监狱与 symlink 规则统一由 `fs_walk` 实现。

use crate::ToolError;
use crate::exec::tool_err;
use crate::fs_response::ResponseBudget;
use crate::fs_walk::{display_path, regular_files};
use serde_json::{Map, Value};
use std::path::Path;

const DEFAULT_MAX_RESULTS: usize = 100;
const MAX_RESULTS: usize = 1_000;
const MAX_QUERY_CHARS: usize = 512;

pub(crate) fn search(root: &Path, input: &Value) -> Result<String, ToolError> {
    let args = SearchFilesInput::parse(input)?;
    let walk = regular_files(root, args.path.as_deref())?;
    let mut matches = ResponseBudget::new();
    let mut truncated = walk.truncated;

    for file in walk.files {
        let name = file
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        if file_name_matches(name, &args.query) {
            if matches.len() == args.max_results {
                truncated = true;
                break;
            }
            let path = display_path(root, &file);
            let encoded = serde_json::to_string(&path).expect("文件路径必须可 JSON 编码");
            if !matches.push_encoded(encoded) {
                truncated = true;
                break;
            }
        }
    }

    Ok(matches.finish(truncated))
}

struct SearchFilesInput {
    query: String,
    path: Option<String>,
    max_results: usize,
}

impl SearchFilesInput {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let object = input
            .as_object()
            .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;
        reject_unknown_fields(object, &["query", "path", "max_results"])?;

        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.chars().count() <= MAX_QUERY_CHARS)
            .ok_or_else(|| tool_err("bad_input", "query 必须是长度不超过 512 的非空字符串"))?
            .to_owned();
        let path = optional_path(object)?;
        let max_results =
            optional_bounded_usize(object, "max_results", DEFAULT_MAX_RESULTS, MAX_RESULTS)?;

        Ok(Self {
            query,
            path,
            max_results,
        })
    }
}

fn reject_unknown_fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ToolError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        return Err(tool_err("bad_input", format!("不支持的参数：{field}")));
    }
    Ok(())
}

fn optional_path(object: &Map<String, Value>) -> Result<Option<String>, ToolError> {
    match object.get("path") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(path)) if !path.trim().is_empty() => Ok(Some(path.trim().to_owned())),
        Some(Value::String(_)) => Err(tool_err("bad_input", "path 不能为空字符串")),
        Some(_) => Err(tool_err("bad_input", "path 必须是字符串")),
    }
}

fn optional_bounded_usize(
    object: &Map<String, Value>,
    name: &str,
    default: usize,
    maximum: usize,
) -> Result<usize, ToolError> {
    match object.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .and_then(|number| usize::try_from(number).ok())
            .filter(|number| (1..=maximum).contains(number))
            .ok_or_else(|| tool_err("bad_input", format!("{name} 必须是 1..={maximum} 的整数"))),
    }
}

fn file_name_matches(name: &str, query: &str) -> bool {
    if query.contains(['*', '?']) {
        glob_matches(
            &query.chars().collect::<Vec<_>>(),
            &name.chars().collect::<Vec<_>>(),
        )
    } else {
        name.contains(query)
    }
}

fn glob_matches(pattern: &[char], text: &[char]) -> bool {
    let (mut pattern_index, mut text_index, mut star_index, mut star_text_index) = (0, 0, None, 0);
    while text_index < text.len() {
        if pattern.get(pattern_index) == Some(&'?')
            || pattern.get(pattern_index) == text.get(text_index)
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern.get(pattern_index) == Some(&'*') {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_text_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_text_index += 1;
            text_index = star_text_index;
        } else {
            return false;
        }
    }
    while pattern.get(pattern_index) == Some(&'*') {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_uses_question_and_star_over_whole_file_name() {
        assert!(file_name_matches("agent-tools.rs", "agent-*.rs"));
        assert!(file_name_matches("ab.txt", "a?.txt"));
        assert!(!file_name_matches("abc.txt", "a?.txt"));
    }

    #[test]
    fn no_wildcard_uses_substring() {
        assert!(file_name_matches("fs_search_files.rs", "search"));
        assert!(!file_name_matches("fs_read.rs", "search"));
    }
}
