//! 六个静态命令工具的模型可见声明。
//!
//! 这些名字与 web-agent 的标准工具名保持一致；它们不进入既有
//! `builtin_specs()`，由 descriptor/权限层显式决定是否暴露。

use agent_core::ToolSpec;
use serde_json::{Value, json};
use std::sync::Arc;

const SHELL_TIMEOUT_MAX_SECS: u64 = 120;
const SHELL_OUTPUT_MAX_BYTES: u64 = 131_072;
const TASK_TIMEOUT_MAX_SECS: u64 = 300;
const TASK_OUTPUT_MAX_BYTES: u64 = 262_144;

/// 所有静态命令声明，顺序固定，方便 descriptor 一次性注册。
pub(crate) fn command_specs() -> Vec<ToolSpec> {
    vec![
        shell_macos_spec(),
        shell_linux_spec(),
        shell_powershell_spec(),
        run_task_spec(),
        run_verification_command_spec(),
        git_diff_review_spec(),
    ]
}

pub(crate) fn shell_macos_spec() -> ToolSpec {
    shell_spec(
        "shell_macos",
        "只在 macOS 上执行一条与当前请求直接相关的非交互 shell 命令。工作目录固定\
         为 workspace 根目录，不能指定 cwd 或 env。优先使用只读检查；不要用它修改\
         文件，文件编辑应改用可撤回的 write_text。命令有明显写入、删除或重定向时会\
         被拒绝，但这不是安全沙箱；仍应把它作为不可逆操作处理。返回文本最多\
         max_output_bytes 字节，截断后请缩小命令范围。",
    )
}

pub(crate) fn shell_linux_spec() -> ToolSpec {
    shell_spec(
        "shell_linux",
        "只在 Linux 上执行一条与当前请求直接相关的非交互 shell 命令。工作目录固定\
         为 workspace 根目录，不能指定 cwd 或 env。优先使用只读检查；不要用它修改\
         文件，文件编辑应改用可撤回的 write_text。命令有明显写入、删除或重定向时会\
         被拒绝，但这不是安全沙箱；仍应把它作为不可逆操作处理。返回文本最多\
         max_output_bytes 字节，截断后请缩小命令范围。",
    )
}

pub(crate) fn shell_powershell_spec() -> ToolSpec {
    shell_spec(
        "shell_powershell",
        "只在 Windows PowerShell 宿主上执行一条非交互命令。当前 Rust 本地执行器尚未\
         接入 PowerShell backend，因此非 Windows 或未接入 backend 时会明确返回\
         unsupported_platform，而不是错误地用 sh 解释 PowerShell。工作目录固定为\
         workspace 根目录；文件修改应使用可撤回的 write_text。返回文本最多\
         max_output_bytes 字节。",
    )
}

pub(crate) fn run_task_spec() -> ToolSpec {
    spec(
        "run_task",
        "运行一个预定义的项目任务，而不是任意 shell。kind 只能是 test、build、lint、\
         typecheck 或 cargo_check；适配器映射为固定 Cargo 命令，不能传参数、cwd 或\
         env。返回文本受 max_output_bytes 限制；非零退出码是任务失败证据，请据此\
         修复或继续检查。",
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["test", "build", "lint", "typecheck", "cargo_check"],
                    "description": "必填；选择最小的预定义检查任务，不能传任意命令。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": TASK_TIMEOUT_MAX_SECS,
                    "default": 300,
                    "description": "可选；任务超时秒数，缺省 300，最大 300。"
                },
                "max_output_bytes": {
                    "type": "integer",
                    "minimum": 256,
                    "maximum": TASK_OUTPUT_MAX_BYTES,
                    "default": 65536,
                    "description": "可选；最多返回的 UTF-8 字节数，缺省 65536。"
                }
            },
            "required": ["kind"],
            "additionalProperties": false
        }),
    )
}

pub(crate) fn run_verification_command_spec() -> ToolSpec {
    spec(
        "run_verification_command",
        "执行一条为当前验收标准取得真实证据的非交互 shell 命令。仅用于 test、lint、\
         typecheck、只读检查或项目自带验证脚本，不应用于编辑文件、启动服务或持续\
         监听。命令在 workspace 根目录执行，固定超时 300 秒，返回最多 131072 字节；\
         非零退出码是有效验收证据，不代表工具没有执行。",
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 32768,
                    "description": "必填；用于验证当前目标的非空、非交互命令。"
                }
            },
            "required": ["command"],
            "additionalProperties": false
        }),
    )
}

pub(crate) fn git_diff_review_spec() -> ToolSpec {
    spec(
        "git_diff_review",
        "只读检查当前 Git 工作区：返回 status、可选 diff stat 和 diff 内容。它不会\
         stage、commit、reset、clean 或修改文件。paths 必须是 workspace 相对路径；\
         大 diff 应传更窄的 paths。base 是不带空白、控制字符或前导 - 的 Git ref/\
         commit。返回文本最多 max_output_bytes 字节，可能带截断标记。",
        json!({
            "type": "object",
            "properties": {
                "paths": {
                    "type": "array",
                    "maxItems": 100,
                    "items": { "type": "string", "minLength": 1, "maxLength": 4096 },
                    "description": "可选；要查看的 workspace 相对文件路径。省略则查看全部变更。"
                },
                "staged": {
                    "type": "boolean",
                    "default": false,
                    "description": "可选；true 时查看 index 中已暂存的 diff。"
                },
                "base": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 255,
                    "description": "可选；与此 ref 或 commit 比较，例如 HEAD~1 或 origin/main。"
                },
                "include_stat": {
                    "type": "boolean",
                    "default": true,
                    "description": "可选；是否返回 diff stat，缺省 true。"
                },
                "max_output_bytes": {
                    "type": "integer",
                    "minimum": 256,
                    "maximum": SHELL_OUTPUT_MAX_BYTES,
                    "default": 24576,
                    "description": "可选；最多返回的 UTF-8 字节数，缺省 24576。"
                }
            },
            "additionalProperties": false
        }),
    )
}

fn shell_spec(name: &'static str, description: &'static str) -> ToolSpec {
    spec(
        name,
        description,
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "minLength": 1,
                    "maxLength": 32768,
                    "description": "必填；非空的非交互命令。不要通过 shell 修改文件。"
                },
                "timeout_secs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": SHELL_TIMEOUT_MAX_SECS,
                    "default": 30,
                    "description": "可选；超时秒数，缺省 30，最大 120。"
                },
                "max_output_bytes": {
                    "type": "integer",
                    "minimum": 256,
                    "maximum": SHELL_OUTPUT_MAX_BYTES,
                    "default": 24576,
                    "description": "可选；最多返回的 UTF-8 字节数，缺省 24576。"
                }
            },
            "required": ["command"],
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
#[path = "command_specs_tests.rs"]
mod tests;
