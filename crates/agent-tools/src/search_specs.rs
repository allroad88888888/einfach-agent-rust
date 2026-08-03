//! workspace 搜索工具的模型输入声明。
//!
//! 两个工具共享「只读、相对 root、确定性结果上限」这个契约，因此声明放在一起；
//! 实际搜索各自在 `fs_search_files` 与 `fs_rg_search`。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

pub(crate) fn search_files_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/search_files"),
        description: Arc::from(
            "按文件名查找 root 之下的常规文件，只读且不会跟随 symlink。query 是必填的\
             文件名子串；含 * 或 ? 时改按完整文件名 glob 匹配。path 是可选的相对\
             root 搜索起点，缺省为 .。max_results 控制最多返回多少个路径。结果按\
             workspace-relative 路径排序，JSON 中的 truncated=true 表示达到结果、\
             遍历或响应字节上限，可缩小 path 后继续。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 512,
                    "description": "必填：文件名子串；含 * 或 ? 时按完整文件名 glob 匹配。"
                },
                "path": {
                    "type": "string",
                    "description": "可选：相对 root 的文件或目录，缺省为 .。"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100,
                    "description": "可选：最多返回的路径数。"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })),
    }
}

pub(crate) fn rg_search_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/rg_search"),
        description: Arc::from(
            "在 root 之下的 UTF-8 文本文件中按字面 needle 搜索，只读且不会跟随\
             symlink。query 必填，不是正则。path 可选且必须相对 root，缺省为 .。\
             每个结果带 workspace-relative path、1-based line/column 和 text；把\
             line 直接传给 fs/read 的 offset 可继续查看。max_line_chars 限制返回\
             的每行字符数。truncated=true 表示达到结果、遍历、单文件读取或响应\
             字节上限，应缩小 path 或更具体 query 后继续。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 4096,
                    "description": "必填：按字面匹配的文本 needle，不支持正则。"
                },
                "path": {
                    "type": "string",
                    "description": "可选：相对 root 的文件或目录，缺省为 .。"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 200,
                    "description": "可选：最多返回的匹配行数。"
                },
                "max_line_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4096,
                    "default": 400,
                    "description": "可选：每条结果 text 的最大字符数，超出时带 …。"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })),
    }
}
