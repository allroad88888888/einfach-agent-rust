//! 标准名称下的可撤回工作区文件工具声明。
//!
//! 这些工具刻意要求 revision：先 read_file，再携带两个路径各自的 token 写入。
//! 因而两个并发 agent 至多一个能提交同一版本，另一个获得可解释的 conflict。

use agent_core::ToolSpec;
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) fn standard_workspace_file_specs() -> Vec<ToolSpec> {
    vec![
        crate::apply_patch_spec::apply_patch_spec(),
        write_file_spec(),
        delete_path_spec(),
        copy_path_spec(),
        move_path_spec(),
        revert_workspace_change_spec(),
    ]
}

fn write_file_spec() -> ToolSpec {
    spec(
        "write_file",
        "原子替换一个 UTF-8 文本文件并记录可撤回 journal。先对同一路径调用 read_file，\
         再把返回的 revision 原样放入 expected_revision；conflict 表示别的 agent 已修改\
         文件，必须重新 read_file 后决定合并。成功 change_id 可由 revert_workspace_change\
         撤回。不会创建父目录，不支持二进制、append 或盲写。",
        text_write_schema(),
    )
}

fn delete_path_spec() -> ToolSpec {
    spec(
        "delete_path",
        "可撤回地删除一个 workspace 内普通 UTF-8 文件。先 read_file 获取 expected_revision；\
         成功 change_id 可由 revert_workspace_change 恢复原文件。目录、symlink 与 recursive\
         删除尚不在此工具契约内，不能用它替代 shell rm。",
        json!({
            "type": "object",
            "properties": {
                "path": path_property(),
                "expected_revision": revision_property()
            },
            "required": ["path", "expected_revision"],
            "additionalProperties": false
        }),
    )
}

fn copy_path_spec() -> ToolSpec {
    path_operation_spec(
        "copy_path",
        "复制一个 UTF-8 文本文件到另一个文件路径，并把 destination 之前的内容记入\
         可撤回 journal。先分别 read_file source 与 destination；把两个 revision 原样传入。\
         任一 revision 过期都会 conflict 且不改动文件。成功 change_id 只撤回 destination。",
    )
}

fn move_path_spec() -> ToolSpec {
    path_operation_spec(
        "move_path",
        "原子地移动一个 UTF-8 文本文件到另一个文件路径。先分别 read_file source 与\
         destination；把两个 revision 原样传入。一个 change_id 覆盖 source 删除与\
         destination 写入；撤回时会先校验两个路径都未被后续修改。",
    )
}

fn path_operation_spec(name: &'static str, description: &'static str) -> ToolSpec {
    spec(
        name,
        description,
        json!({
            "type": "object",
            "properties": {
                "source": path_property(),
                "destination": path_property(),
                "expected_source_revision": revision_property(),
                "expected_destination_revision": revision_property()
            },
            "required": [
                "source",
                "destination",
                "expected_source_revision",
                "expected_destination_revision"
            ],
            "additionalProperties": false
        }),
    )
}

fn revert_workspace_change_spec() -> ToolSpec {
    spec(
        "revert_workspace_change",
        "撤回一个 apply_patch、write_file、delete_path、copy_path 或 move_path 的成功结果。传入\
         change_id；只有所有受影响路径仍保持该变更写入后的 revision 才会恢复 preimage。\
         conflict 表示后续修改已发生，绝不覆盖。一个 change_id 只能成功撤回一次。",
        json!({
            "type": "object",
            "properties": {
                "change_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "description": "必填：变更成功结果中的 change_id，必须原样传入。"
                }
            },
            "required": ["change_id"],
            "additionalProperties": false
        }),
    )
}

fn text_write_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": path_property(),
            "content": {
                "type": "string",
                "maxLength": 1048576,
                "description": "必填：替换后的完整 UTF-8 文本，最多 1 MiB（按 UTF-8 字节计）。"
            },
            "expected_revision": revision_property()
        },
        "required": ["path", "content", "expected_revision"],
        "additionalProperties": false
    })
}

fn path_property() -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": "相对 workspace root 的普通文件路径；不允许绝对路径、.. 或 symlink。"
    })
}

fn revision_property() -> Value {
    json!({
        "type": "string",
        "minLength": 9,
        "maxLength": 79,
        "description": "对应路径最近一次 read_file 返回的 revision，必须原样传入。"
    })
}

fn spec(name: &'static str, description: &'static str, schema: Value) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(schema),
    }
}

#[cfg(test)]
#[path = "workspace_standard_specs_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "workspace_standard_specs_perf_tests.rs"]
mod perf_tests;
