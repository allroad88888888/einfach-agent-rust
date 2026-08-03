//! 工具声明：内置只读集 `srv:fs/read`、`srv:fs/list`，以及独立声明的
//! `srv:shell/exec`（issue 020）。
//!
//! **顺序固定**（fs/read 在前，fs/list 在后，见 issue 013）——`builtin_specs()`
//! 装进 `Vec`，不是无序容器，顺序本身就是稳定序列化的一部分（红线 11）。
//! schema 用 `serde_json::json!` 构造：`Value::Object` 的底层是 `BTreeMap`，
//! key 按字典序输出，同一份 schema 序列化两次逐字节相同——这是本仓依赖的
//! `serde_json` 默认行为，不是巧合（见 agent-core `value/tool.rs` 顶部注释）。
//!
//! description 用中文写清楚参数语义：这是模型看的，不是给人看的 doc comment。
//!
//! `shell_spec()` **故意不进 `builtin_specs()`**：`srv:shell/exec` 是
//! `Reversibility::Irreversible`，没有 undo 屏障挡着默认开着是数据事故，
//! 020 的范围只做工具本体，集成阶段（连同 undo 屏障一起）才显式把它加进
//! 某个工具表（见 `docs/issues/020-shell-tool.md`「范围裁决」）。

use agent_core::ToolSpec;
use serde_json::json;
use std::sync::Arc;

/// 内置工具表，顺序固定：`[fs/read, fs/list]`。
pub(crate) fn builtin_specs() -> Vec<ToolSpec> {
    vec![fs_read_spec(), fs_list_spec()]
}

fn fs_read_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/read"),
        description: Arc::from(
            "读取 root 之下一个文件的内容，返回选中行的原文（不带行号）。\
             path：相对 root 的文件路径，必填。offset：起始行号（1-based，\
             从第几行开始读），可选，缺省从第 1 行开始；超过文件总行数时返回\
             空字符串而不是报错。limit：最多返回的行数，可选，缺省读到文件\
             末尾。文件较大时用 offset/limit 分批读取，避免一次拿到的内容\
             被截断。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "相对 root 的文件路径，必填。"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "起始行号（1-based），可选，缺省为 1。"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "最多返回的行数，可选，缺省读到文件末尾。"
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })),
    }
}

/// `srv:shell/exec` 的声明（issue 020）。`Location::Server` /
/// `Reversibility::Irreversible` 不是这个类型的字段——那两个维度由调用方的
/// 工具表决定（`docs/TOOLS.md`），这里只声明模型看到的名字/schema。
pub(crate) fn shell_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:shell/exec"),
        description: Arc::from(
            "在服务端执行一条 shell 命令（sh -c）。工作目录固定为项目根目录，\
             不能 cd 到根目录之外，但命令内容本身不受限——它能修改、删除任何\
             这个进程有权限碰到的东西，属于不可逆操作，执行前请确认真的要跑。\
             cmd：要执行的 shell 命令，必填。timeout_secs：超时秒数，可选，\
             缺省 30，范围 1~300；超时会强制终止整条命令（含它开出的所有子\
             进程）。返回内容是标准输出；有标准错误会在末尾追加\
             \\n[stderr]\\n<内容>；退出码非 0 会在末尾追加\
             \\n[exit code: N]——命令跑起来但失败了不算工具调用失败，看这两个\
             标记自己判断要不要紧。只有命令根本起不来或超时才会报错。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "cmd": {
                    "type": "string",
                    "description": "要执行的 shell 命令，必填。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 300,
                    "description": "超时秒数，可选，缺省 30，范围 1~300。"
                }
            },
            "required": ["cmd"],
            "additionalProperties": false
        })),
    }
}

fn fs_list_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from("srv:fs/list"),
        description: Arc::from(
            "列出 root 之下某个目录的直接子项（不递归）。每行一个条目，\
             按名字典序排序；目录名带尾部 /，文件名不带。path：相对 root 的\
             目录路径，可选，缺省为 \".\"（即 root 本身）。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "相对 root 的目录路径，可选，缺省为 \".\"。"
                }
            },
            "additionalProperties": false
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_is_fixed_read_then_list() {
        let specs = builtin_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(&*specs[0].name, "srv:fs/read");
        assert_eq!(&*specs[1].name, "srv:fs/list");
    }

    #[test]
    fn fs_read_schema_has_required_path_and_int_bounds() {
        let specs = builtin_specs();
        let schema = &specs[0].schema;
        assert_eq!(schema["required"], json!(["path"]));
        assert_eq!(schema["properties"]["offset"]["minimum"], json!(1));
        assert_eq!(schema["properties"]["limit"]["minimum"], json!(1));
        assert_eq!(schema["properties"]["path"]["type"], json!("string"));
    }

    #[test]
    fn fs_list_schema_path_is_optional() {
        let specs = builtin_specs();
        let schema = &specs[1].schema;
        assert!(schema.get("required").is_none());
        assert_eq!(schema["properties"]["path"]["type"], json!("string"));
    }

    /// `shell_spec()` 不在 `builtin_specs()` 里——它是独立声明的，见本文件
    /// 顶部注释和 020「范围裁决」。
    #[test]
    fn shell_spec_is_not_in_builtin_table() {
        let specs = builtin_specs();
        let names: Vec<&str> = specs.iter().map(|s| &*s.name).collect();
        assert!(!names.contains(&"srv:shell/exec"));
    }

    #[test]
    fn shell_spec_schema_requires_cmd_and_bounds_timeout() {
        let spec = shell_spec();
        assert_eq!(&*spec.name, "srv:shell/exec");
        let schema = &spec.schema;
        assert_eq!(schema["required"], json!(["cmd"]));
        assert_eq!(schema["properties"]["cmd"]["type"], json!("string"));
        assert_eq!(schema["properties"]["timeout_secs"]["minimum"], json!(1));
        assert_eq!(schema["properties"]["timeout_secs"]["maximum"], json!(300));
    }

    /// 红线 11 最小实检：同一份表序列化两次逐字节相同。
    #[test]
    fn serializes_byte_identical_across_calls() {
        let a = serde_json::to_vec(&builtin_specs()).unwrap();
        let b = serde_json::to_vec(&builtin_specs()).unwrap();
        assert_eq!(a, b);
    }
}
