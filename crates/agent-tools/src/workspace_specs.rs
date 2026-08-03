//! 可撤回工作区工具的模型输入声明。
//!
//! 三步协议故意独立：先 inspect 拿 revision，再 write，最后按 change_id revert。
//! 这让模型能从每一次工具结果中得到下一步唯一需要保存的值，避免猜测状态。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn inspect_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/inspect"),
        description: Arc::from(
            "读取一个相对 root 的文本文件状态，返回 path、exists 和 revision。写入前必须先\
             调用此工具，并把同一 path 返回的 revision 原样作为 write_text 的\
             expected_revision；不要自行构造 revision。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "必填：相对 root 的普通文件路径，不支持绝对路径、.. 或 symlink。"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })),
    }
}

pub(crate) fn write_text_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/write_text"),
        description: Arc::from(
            "原子写入一个 UTF-8 文本文件并保存可撤回 journal。必须先对同一路径调用\
             inspect，再原样传入其 revision；若返回 conflict，重新 inspect 后再决定\
             如何合并，绝不盲目重试。成功结果中的 change_id 可供 revert_change 撤回。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "必填：相对 root 的普通文件路径，不支持绝对路径、.. 或 symlink。"
                },
                "content": {
                    "type": "string",
                    "maxLength": 1048576,
                    "description": "必填：替换后的完整 UTF-8 文本，最多 1 MiB（按 UTF-8 字节计，中文等多字节字符会更少）；不会自动创建父目录。"
                },
                "expected_revision": {
                    "type": "string",
                    "minLength": 9,
                    "maxLength": 79,
                    "description": "必填：同一路径最近一次 inspect 返回的 revision，必须原样传入。"
                }
            },
            "required": ["path", "content", "expected_revision"],
            "additionalProperties": false
        })),
    }
}

pub(crate) fn revert_change_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:workspace/revert_change"),
        description: Arc::from(
            "撤回一次 write_text 成功写入。传入该次结果的 change_id；只有目标仍保持\
             当时写入的 revision 才会恢复 preimage，外部改动后返回 conflict 而不覆盖。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "change_id": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 128,
                    "description": "必填：write_text 成功结果中的 change_id。"
                }
            },
            "required": ["change_id"],
            "additionalProperties": false
        })),
    }
}
