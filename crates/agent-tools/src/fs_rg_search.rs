//! `srv:fs/rg_search`：按字面 needle 搜索 workspace 文本文件。
//!
//! 这个实现不启动外部 `rg` 进程。每个文件最多读取固定字节数，非 UTF-8 或
//! 超过上限的文件会跳过并把结果标为 `truncated`；因此资源边界不依赖 wall-clock。

use crate::ToolError;
use crate::exec::tool_err;
use crate::fs_response::ResponseBudget;
use crate::fs_walk::{display_path, regular_files};
use serde_json::{Map, Value, json};
use std::io::Read;
use std::path::Path;

const DEFAULT_MAX_RESULTS: usize = 200;
const MAX_RESULTS: usize = 1_000;
const DEFAULT_MAX_LINE_CHARS: usize = 400;
const MAX_LINE_CHARS: usize = 4_096;
const MAX_QUERY_CHARS: usize = 4_096;
const MAX_FILE_BYTES: usize = 1_048_576;

pub(crate) fn search(root: &Path, input: &Value) -> Result<String, ToolError> {
    let args = RgSearchInput::parse(input)?;
    let walk = regular_files(root, args.path.as_deref())?;
    let mut matches = ResponseBudget::new();
    let mut truncated = walk.truncated;
    let mut result_limit_reached = false;

    for file in walk.files {
        let Some(content) = read_utf8_file(&file) else {
            truncated = true;
            continue;
        };

        for (index, line) in content.lines().enumerate() {
            let Some(byte_column) = line.find(&args.query) else {
                continue;
            };
            if matches.len() == args.max_results {
                truncated = true;
                result_limit_reached = true;
                break;
            }
            let (text, line_truncated) = clip_line(line, args.max_line_chars);
            let hit = json!({
                "path": display_path(root, &file),
                "line": index + 1,
                "column": line[..byte_column].chars().count() + 1,
                "text": text,
                "line_truncated": line_truncated,
            });
            let encoded = serde_json::to_string(&hit).expect("搜索命中必须可 JSON 编码");
            if !matches.push_encoded(encoded) {
                truncated = true;
                result_limit_reached = true;
                break;
            }
        }
        if result_limit_reached {
            break;
        }
    }

    Ok(matches.finish(truncated))
}

struct RgSearchInput {
    query: String,
    path: Option<String>,
    max_results: usize,
    max_line_chars: usize,
}

impl RgSearchInput {
    fn parse(input: &Value) -> Result<Self, ToolError> {
        let object = input
            .as_object()
            .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;
        reject_unknown_fields(object, &["query", "path", "max_results", "max_line_chars"])?;

        let query = object
            .get("query")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.chars().count() <= MAX_QUERY_CHARS)
            .ok_or_else(|| tool_err("bad_input", "query 必须是长度不超过 4096 的非空字符串"))?
            .to_owned();

        Ok(Self {
            query,
            path: optional_path(object)?,
            max_results: optional_bounded_usize(
                object,
                "max_results",
                DEFAULT_MAX_RESULTS,
                MAX_RESULTS,
            )?,
            max_line_chars: optional_bounded_usize(
                object,
                "max_line_chars",
                DEFAULT_MAX_LINE_CHARS,
                MAX_LINE_CHARS,
            )?,
        })
    }
}

fn read_utf8_file(path: &Path) -> Option<String> {
    let mut bytes = Vec::with_capacity(MAX_FILE_BYTES.min(8_192));
    let mut file = std::fs::File::open(path)
        .ok()?
        .take((MAX_FILE_BYTES + 1) as u64);
    file.read_to_end(&mut bytes).ok()?;
    if bytes.len() > MAX_FILE_BYTES {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn clip_line(line: &str, maximum: usize) -> (String, bool) {
    let mut chars = line.chars();
    let head: String = chars.by_ref().take(maximum).collect();
    if chars.next().is_some() {
        (format!("{head}…"), true)
    } else {
        (head, false)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_line_preserves_unicode_boundaries() {
        assert_eq!(clip_line("甲乙丙", 2), ("甲乙…".to_string(), true));
        assert_eq!(clip_line("甲乙", 2), ("甲乙".to_string(), false));
    }
}
