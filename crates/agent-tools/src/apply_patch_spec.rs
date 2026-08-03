//! 标准 `apply_patch` 的闭合、可恢复文本文件 schema。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn apply_patch_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("apply_patch"),
        description: Arc::from(
            "将 1 到 16 个 UTF-8 文本文件变更作为一个可撤回事务提交。每个 path 只能出现一次；\
             add_file 要求文件不存在，delete_file 与 overwrite_file 必须回传 oldContent 或\
             expectedContentHash，replace 必须声明 oldText 命中的精确次数。dryRun 只做完整\
             校验与预览，不写文件。成功的 change_id 可由 revert_workspace_change 原子撤回\
             全部文件；任一路径被另一 agent 修改会 conflict，整批不落盘。仅支持普通文件，\
             不创建目录、不跟随 symlink、不支持二进制或 executable。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 16,
                    "items": { "oneOf": operation_schemas() },
                    "description": "必填：不同 path 的批量文本操作，按数组顺序执行。"
                },
                "dryRun": {
                    "type": "boolean",
                    "default": false,
                    "description": "可选：true 时只校验和返回将要变更的路径，不写入 journal 或文件。"
                }
            },
            "required": ["operations"],
            "additionalProperties": false
        })),
    }
}

fn operation_schemas() -> Vec<serde_json::Value> {
    vec![
        add_file_schema(),
        delete_file_schema(),
        replace_schema(),
        overwrite_file_schema(),
    ]
}

fn add_file_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "const": "add_file" },
            "path": path_property(),
            "content": content_property("写入的新文件完整内容。")
        },
        "required": ["type", "path", "content"],
        "additionalProperties": false
    })
}

fn delete_file_schema() -> serde_json::Value {
    guarded_file_schema(
        "delete_file",
        None,
        "删除前完整内容；与 expectedContentHash 二选一。",
    )
}

fn overwrite_file_schema() -> serde_json::Value {
    guarded_file_schema(
        "overwrite_file",
        Some(content_property("覆盖后的完整内容。")),
        "覆盖前完整内容；与 expectedContentHash 二选一。",
    )
}

fn guarded_file_schema(
    kind: &str,
    content: Option<serde_json::Value>,
    old_content_description: &str,
) -> serde_json::Value {
    let mut properties = serde_json::Map::from_iter([
        ("type".to_owned(), json!({ "const": kind })),
        ("path".to_owned(), path_property()),
        (
            "oldContent".to_owned(),
            content_property(old_content_description),
        ),
        (
            "expectedContentHash".to_owned(),
            json!({
                "type": "string",
                "pattern": "^sha256:[0-9a-f]{64}$",
                "description": "oldContent 的紧凑替代；必须是当前文件的 SHA-256。"
            }),
        ),
    ]);
    let mut required = vec![json!("type"), json!("path")];
    if let Some(content) = content {
        properties.insert("content".to_owned(), content);
        required.push(json!("content"));
    }
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "oneOf": [
            { "required": ["oldContent"] },
            { "required": ["expectedContentHash"] }
        ],
        "additionalProperties": false
    })
}

fn replace_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "type": { "const": "replace" },
            "path": path_property(),
            "oldText": content_property("必填：要精确查找的非空文本。"),
            "newText": content_property("必填：每处匹配替换后的文本，可为空。"),
            "expectedReplacements": {
                "type": "integer",
                "minimum": 1,
                "maximum": 1048576,
                "default": 1,
                "description": "可选：oldText 在当前文件中应出现的非重叠次数；不匹配即 conflict。"
            }
        },
        "required": ["type", "path", "oldText", "newText"],
        "additionalProperties": false
    })
}

fn path_property() -> serde_json::Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "相对 workspace root 的普通文件路径；不允许绝对路径、.. 或 symlink。"
    })
}

fn content_property(description: &str) -> serde_json::Value {
    json!({
        "type": "string",
        "maxLength": 1048576,
        "description": description
    })
}

#[cfg(test)]
#[path = "apply_patch_spec_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "apply_patch_spec_perf_tests.rs"]
mod perf_tests;
