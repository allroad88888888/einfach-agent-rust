//! 可撤回工作区工具的 JSON 适配层。

use crate::ToolError;
use crate::workspace::patch_input;
use crate::workspace::revision::Revision;
use crate::workspace::transaction::WorkspaceTransactionCoordinator;
use serde_json::{Map, Value, json};

/// 处理标准 `read_file`。响应中的 revision 与完整文件内容来自同一次读取，可直接
/// 作为 write/delete/copy/move 的 optimistic-concurrency 前置条件。
pub(crate) fn read_file(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(input, &["path", "offset", "limit"])?;
    let path = required_non_empty_string(object, "path")?;
    let offset = optional_positive_integer(object, "offset")?;
    let limit = optional_positive_integer(object, "limit")?;
    let (path, exists, content, revision) = coordinator.read_text(path)?;
    encode(json!({
        "path": path,
        "exists": exists,
        "content": crate::fs_read::select_lines(&content, offset, limit),
        "revision": revision.as_str(),
    }))
}

/// 处理 `srv:fs/inspect`，返回下一次写入所需的 revision。
pub(crate) fn inspect(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(input, &["path"])?;
    let path = required_non_empty_string(object, "path")?;
    let (path, exists, revision) = coordinator.inspect(path)?;
    encode(json!({ "path": path, "exists": exists, "revision": revision.as_str() }))
}

/// 处理 `srv:fs/write_text`，要求调用方携带 inspect/read 到的 revision。
pub(crate) fn write_text(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(input, &["path", "content", "expected_revision"])?;
    let path = required_non_empty_string(object, "path")?;
    let content = required_string(object, "content")?;
    let expected =
        Revision::from_expected_token(required_non_empty_string(object, "expected_revision")?)?;
    let change = coordinator.write_text(path, &expected, content)?;
    encode(json!({
        "change_id": change.change_id(),
        "before_revision": change.before_revision().as_str(),
        "after_revision": change.after_revision().as_str(),
    }))
}

/// 处理标准 `delete_path`。当前实现只接受普通文本文件；目录递归删除需要单独的
/// 多文件 snapshot journal，不能把 `recursive` 悄悄降级为不可撤回的 `rm -r`。
pub(crate) fn delete_file(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(input, &["path", "expected_revision"])?;
    let path = required_non_empty_string(object, "path")?;
    let expected = expected_revision(object, "expected_revision")?;
    let change = coordinator.delete_file(path, &expected)?;
    encode_change(&change)
}

/// 处理标准 `copy_path`。两个路径都必须带最近 read_file 返回的 revision。
pub(crate) fn copy_file(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(
        input,
        &[
            "source",
            "destination",
            "expected_source_revision",
            "expected_destination_revision",
        ],
    )?;
    let source = required_non_empty_string(object, "source")?;
    let destination = required_non_empty_string(object, "destination")?;
    let expected_source = expected_revision(object, "expected_source_revision")?;
    let expected_destination = expected_revision(object, "expected_destination_revision")?;
    let change =
        coordinator.copy_file(source, &expected_source, destination, &expected_destination)?;
    encode_change(&change)
}

/// 处理标准 `move_path`。成功的 change_id 同时涵盖 source 删除与 destination
/// 覆盖，revert 会在两个路径均未被后续修改时一并恢复。
pub(crate) fn move_file(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(
        input,
        &[
            "source",
            "destination",
            "expected_source_revision",
            "expected_destination_revision",
        ],
    )?;
    let source = required_non_empty_string(object, "source")?;
    let destination = required_non_empty_string(object, "destination")?;
    let expected_source = expected_revision(object, "expected_source_revision")?;
    let expected_destination = expected_revision(object, "expected_destination_revision")?;
    let change =
        coordinator.move_file(source, &expected_source, destination, &expected_destination)?;
    encode(json!({
        "change_id": change.change_id(),
        "source_before_revision": change.before_revision().as_str(),
        "source_after_revision": change.after_revision().as_str(),
        "destination_after_revision": change.before_revision().as_str(),
    }))
}

/// 处理 `srv:workspace/revert_change`，仅撤回仍保有写入 revision 的变更。
pub(crate) fn revert_change(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let object = object(input, &["change_id"])?;
    let revision = coordinator.revert(required_non_empty_string(object, "change_id")?)?;
    encode(json!({ "revision": revision.as_str() }))
}

/// 处理标准 `apply_patch`。解析层拒绝未知字段、重复目标与无 guard 的 destructive
/// 操作；协调器把整个批次写入同一份 journal，因而用一个 change_id 即可撤回。
pub(crate) fn apply_patch(
    coordinator: &WorkspaceTransactionCoordinator,
    input: &Value,
) -> Result<String, ToolError> {
    let patch = patch_input::parse(input)?;
    let result = coordinator.apply_patch(&patch)?;
    encode(json!({
        "change_id": result.change_id,
        "changed_files": result.changed_files,
        "dry_run": result.dry_run,
    }))
}

fn object<'a>(input: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, ToolError> {
    let object = input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(tool_err("bad_input", format!("不支持字段：{key}")));
        }
    }
    Ok(object)
}

fn required_string<'a>(object: &'a Map<String, Value>, key: &str) -> Result<&'a str, ToolError> {
    object
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| tool_err("bad_input", format!("{key} 是必填字符串")))
}

fn required_non_empty_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a str, ToolError> {
    let value = required_string(object, key)?;
    if value.is_empty() {
        Err(tool_err("bad_input", format!("{key} 不能为空")))
    } else {
        Ok(value)
    }
}

fn expected_revision(object: &Map<String, Value>, key: &str) -> Result<Revision, ToolError> {
    Revision::from_expected_token(required_non_empty_string(object, key)?)
}

fn optional_positive_integer(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, ToolError> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|number| *number >= 1)
            .map(Some)
            .ok_or_else(|| tool_err("bad_input", format!("{key} 必须是 ≥1 的整数"))),
    }
}

fn encode_change(
    change: &crate::workspace::transaction::WorkspaceChange,
) -> Result<String, ToolError> {
    encode(json!({
        "change_id": change.change_id(),
        "before_revision": change.before_revision().as_str(),
        "after_revision": change.after_revision().as_str(),
    }))
}

fn encode(value: Value) -> Result<String, ToolError> {
    serde_json::to_string(&value)
        .map_err(|error| tool_err("internal_error", format!("无法编码工具结果：{error}")))
}

fn tool_err(code: &'static str, message: impl Into<String>) -> ToolError {
    ToolError {
        code: code.into(),
        message: message.into().into(),
    }
}
