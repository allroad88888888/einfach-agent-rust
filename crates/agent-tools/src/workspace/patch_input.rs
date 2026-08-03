//! `apply_patch` 的闭合 JSON 输入解析。

use crate::ToolError;
use crate::workspace::batch_journal::MAX_BATCH_ENTRIES;
use crate::workspace::text_file::validate_text;
use crate::workspace::transaction::tool_err;
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Debug)]
pub(crate) struct PatchInput {
    pub(crate) operations: Vec<PatchOperation>,
    pub(crate) dry_run: bool,
}

#[derive(Debug)]
pub(crate) enum PatchOperation {
    AddFile {
        path: String,
        content: String,
    },
    DeleteFile {
        path: String,
        guard: ContentGuard,
    },
    Replace {
        path: String,
        old_text: String,
        new_text: String,
        expected_replacements: usize,
    },
    OverwriteFile {
        path: String,
        content: String,
        guard: ContentGuard,
    },
}

#[derive(Debug)]
pub(crate) enum ContentGuard {
    Exact(String),
    Sha256(String),
}

impl PatchOperation {
    pub(crate) fn path(&self) -> &str {
        match self {
            Self::AddFile { path, .. }
            | Self::DeleteFile { path, .. }
            | Self::Replace { path, .. }
            | Self::OverwriteFile { path, .. } => path,
        }
    }
}

pub(crate) fn parse(input: &Value) -> Result<PatchInput, ToolError> {
    let object = object(input, &["operations", "dryRun"])?;
    let dry_run = optional_bool(object, "dryRun")?.unwrap_or(false);
    let values = object
        .get("operations")
        .and_then(Value::as_array)
        .ok_or_else(|| tool_err("bad_input", "operations 是必填数组"))?;
    if values.is_empty() || values.len() > MAX_BATCH_ENTRIES {
        return Err(tool_err(
            "bad_input",
            format!("operations 必须有 1 到 {MAX_BATCH_ENTRIES} 项"),
        ));
    }
    let operations = values
        .iter()
        .enumerate()
        .map(|(index, value)| parse_operation(value, index))
        .collect::<Result<Vec<_>, _>>()?;
    ensure_distinct_paths(&operations)?;
    Ok(PatchInput {
        operations,
        dry_run,
    })
}

fn parse_operation(value: &Value, index: usize) -> Result<PatchOperation, ToolError> {
    let object = value
        .as_object()
        .ok_or_else(|| tool_err("bad_input", format!("operations[{index}] 必须是对象")))?;
    let kind = required_string(object, "type", index)?;
    match kind {
        "add_file" => {
            reject_unknown(object, &["type", "path", "content"])?;
            Ok(PatchOperation::AddFile {
                path: required_path(object, index)?,
                content: required_content(object, "content", index)?,
            })
        }
        "delete_file" => {
            reject_unknown(
                object,
                &["type", "path", "oldContent", "expectedContentHash"],
            )?;
            Ok(PatchOperation::DeleteFile {
                path: required_path(object, index)?,
                guard: content_guard(object, index)?,
            })
        }
        "replace" => {
            reject_unknown(
                object,
                &["type", "path", "oldText", "newText", "expectedReplacements"],
            )?;
            let old_text = required_content(object, "oldText", index)?;
            if old_text.is_empty() {
                return Err(tool_err(
                    "bad_input",
                    format!("operations[{index}].oldText 不能为空"),
                ));
            }
            Ok(PatchOperation::Replace {
                path: required_path(object, index)?,
                old_text,
                new_text: required_content(object, "newText", index)?,
                expected_replacements: optional_positive(object, "expectedReplacements", index)?
                    .unwrap_or(1),
            })
        }
        "overwrite_file" => {
            reject_unknown(
                object,
                &[
                    "type",
                    "path",
                    "content",
                    "oldContent",
                    "expectedContentHash",
                ],
            )?;
            Ok(PatchOperation::OverwriteFile {
                path: required_path(object, index)?,
                content: required_content(object, "content", index)?,
                guard: content_guard(object, index)?,
            })
        }
        _ => Err(tool_err(
            "bad_input",
            format!("operations[{index}].type 不支持：{kind}"),
        )),
    }
}

fn content_guard(object: &Map<String, Value>, index: usize) -> Result<ContentGuard, ToolError> {
    let exact = optional_string(object, "oldContent", index)?;
    let hash = optional_string(object, "expectedContentHash", index)?;
    match (exact, hash) {
        (Some(_), Some(_)) => Err(tool_err(
            "bad_input",
            format!("operations[{index}] 只能传 oldContent 或 expectedContentHash 之一"),
        )),
        (Some(value), None) => {
            validate_text(value.as_bytes())?;
            Ok(ContentGuard::Exact(value))
        }
        (None, Some(value)) if valid_sha256(&value) => Ok(ContentGuard::Sha256(value)),
        (None, Some(_)) => Err(tool_err(
            "bad_input",
            format!("operations[{index}].expectedContentHash 必须是 sha256:<64 小写十六进制>"),
        )),
        (None, None) => Err(tool_err(
            "bad_input",
            format!("operations[{index}] 必须传 oldContent 或 expectedContentHash 防止盲写"),
        )),
    }
}

fn object<'a>(input: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, ToolError> {
    let object = input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;
    reject_unknown(object, allowed)?;
    Ok(object)
}

fn reject_unknown(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ToolError> {
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(tool_err("bad_input", format!("不支持字段：{key}")))
    } else {
        Ok(())
    }
}

fn required_path(object: &Map<String, Value>, index: usize) -> Result<String, ToolError> {
    let path = required_string(object, "path", index)?;
    if path.is_empty() {
        Err(tool_err(
            "bad_input",
            format!("operations[{index}].path 不能为空"),
        ))
    } else {
        Ok(path.to_owned())
    }
}

fn required_content(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<String, ToolError> {
    let value = required_string(object, key, index)?.to_owned();
    validate_text(value.as_bytes())?;
    Ok(value)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<&'a str, ToolError> {
    object.get(key).and_then(Value::as_str).ok_or_else(|| {
        tool_err(
            "bad_input",
            format!("operations[{index}].{key} 是必填字符串"),
        )
    })
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<Option<String>, ToolError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(tool_err(
            "bad_input",
            format!("operations[{index}].{key} 必须是字符串"),
        )),
    }
}

fn optional_positive(
    object: &Map<String, Value>,
    key: &str,
    index: usize,
) -> Result<Option<usize>, ToolError> {
    match object.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|value| value.try_into().ok())
            .filter(|value: &usize| *value > 0)
            .ok_or_else(|| {
                tool_err(
                    "bad_input",
                    format!("operations[{index}].{key} 必须是正整数"),
                )
            })
            .map(Some),
    }
}

fn optional_bool(object: &Map<String, Value>, key: &str) -> Result<Option<bool>, ToolError> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(tool_err("bad_input", format!("{key} 必须是布尔值"))),
    }
}

fn ensure_distinct_paths(operations: &[PatchOperation]) -> Result<(), ToolError> {
    let paths: BTreeSet<_> = operations.iter().map(PatchOperation::path).collect();
    if paths.len() == operations.len() {
        Ok(())
    } else {
        Err(tool_err(
            "bad_input",
            "同一次 apply_patch 不能重复修改同一路径",
        ))
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
#[path = "patch_input_tests.rs"]
mod tests;
