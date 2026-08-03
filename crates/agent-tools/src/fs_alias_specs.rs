//! web-agent 标准只读文件工具名的声明。
//!
//! 这里仅描述模型可见的标准名；除 `read_file` 增加 revision 回执外，执行阶段
//! 必须转发到同语义的既有 `srv:fs/*` 实现，不能复制或分叉文件系统逻辑。

use agent_core::ToolSpec;
use serde_json::{Value, json};
use std::sync::Arc;

/// 标准只读文件工具，顺序固定为 read、list、name-search、text-search。
pub(crate) fn standard_readonly_file_specs() -> Vec<ToolSpec> {
    vec![
        read_file_spec(),
        list_files_spec(),
        search_files_spec(),
        rg_search_spec(),
    ]
}

/// `read_file` 返回页面内容及完整文件的 revision。
pub(crate) fn read_file_spec() -> ToolSpec {
    spec(
        "read_file",
        "只读地读取 workspace root 下一个文本文件的原文。path 是必填的相对路径；\
         offset 是 1-based 起始行，limit 是最多返回的行数。遇到大文件时先用小的\
         limit 分页；把 rg_search 返回的 line 直接作为 offset。结果总会带完整文件\
         的 revision；编辑时原样传给 write_file/delete_path 等 expected_revision，\
         无需调用额外的内部 inspect 工具。文件不存在时返回 exists=false、空 content\
         和 revision=absent:v1，后者可直接用于安全创建。只支持最多 1 MiB 的 UTF-8\
         普通文件，不能读取 root 外路径或穿过 symlink。",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "必填：相对 workspace root 的普通文件路径。"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "可选：1-based 起始行，缺省为第 1 行。"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "可选：最多返回的行数；大文件必须分页读取。"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        }),
    )
}

/// `list_files` 必须转发到既有 `srv:fs/list`。
pub(crate) fn list_files_spec() -> ToolSpec {
    spec(
        "list_files",
        "只读地列出 workspace root 下目录的直接子项，不递归。path 可选，缺省为 .；\
         结果按名称稳定排序，目录带 /。执行时转发到 srv:fs/list，不能列出 root 外\
         路径或跟随 symlink。目录很大时先缩小 path 再继续。",
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "default": ".",
                    "description": "可选：相对 workspace root 的目录，缺省为 .。"
                }
            },
            "additionalProperties": false
        }),
    )
}

/// `search_files` 必须转发到既有 `srv:fs/search_files`。
pub(crate) fn search_files_spec() -> ToolSpec {
    spec(
        "search_files",
        "只读地按文件名查找 workspace root 下的常规文件。query 是必填文件名子串；\
         含 * 或 ? 时按完整文件名 glob 匹配。结果按相对路径稳定排序，truncated=true\
         表示碰到结果、遍历或响应预算，需缩小 path 或 query。执行时转发到\
         srv:fs/search_files，绝不跟随 symlink。",
        json!({
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
                    "minLength": 1,
                    "description": "可选：相对 workspace root 的搜索起点，缺省为 .。"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 100,
                    "description": "可选：最多返回 1000 个相对路径。"
                }
            },
            "required": ["query"],
            "additionalProperties": false
        }),
    )
}

/// `rg_search` 必须转发到既有 `srv:fs/rg_search`。
pub(crate) fn rg_search_spec() -> ToolSpec {
    spec(
        "rg_search",
        "只读地在 workspace root 下 UTF-8 文本文件中按字面 query 搜索，不支持正则。\
         每个命中含 path、1-based line、column 与 text；将 line 传给 read_file.offset\
         可读上下文。truncated=true 表示碰到结果、遍历、单文件读取或响应预算。执行\
         时转发到 srv:fs/rg_search，绝不跟随 symlink。",
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 4096,
                    "description": "必填：按字面匹配的文本，不支持正则。"
                },
                "path": {
                    "type": "string",
                    "minLength": 1,
                    "description": "可选：相对 workspace root 的文件或目录，缺省为 .。"
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 1000,
                    "default": 200,
                    "description": "可选：最多返回 1000 条匹配行。"
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
        }),
    )
}

fn spec(name: &'static str, description: &'static str, schema: Value) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(schema),
    }
}

#[cfg(test)]
#[path = "fs_alias_specs_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "fs_alias_specs_perf_tests.rs"]
mod perf_tests;
