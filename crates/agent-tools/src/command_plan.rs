//! 静态命令工具的闭合输入解析与 shell 执行计划构造。

use crate::ToolError;
use crate::exec::tool_err;
use serde_json::{Map, Value};

const SHELL_TIMEOUT_DEFAULT: u64 = 30;
const SHELL_TIMEOUT_MAX: u64 = 120;
const TASK_TIMEOUT_DEFAULT: u64 = 300;
const TASK_TIMEOUT_MAX: u64 = 300;
const VERIFY_TIMEOUT_SECS: u64 = 300;
const SHELL_OUTPUT_DEFAULT: usize = 24_576;
const SHELL_OUTPUT_MAX: usize = 131_072;
const TASK_OUTPUT_DEFAULT: usize = 65_536;
const TASK_OUTPUT_MAX: usize = 262_144;
const VERIFY_OUTPUT_MAX: usize = 131_072;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CommandPlan {
    pub(crate) command: String,
    pub(crate) timeout_secs: u64,
    pub(crate) max_output_bytes: usize,
    pub(crate) platform: Platform,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Platform {
    Macos,
    Linux,
    PowerShell,
    PosixHost,
}

pub(crate) fn plan(tool: &str, input: &Value) -> Result<CommandPlan, ToolError> {
    match tool {
        "shell_macos" => shell_plan(input, Platform::Macos),
        "shell_linux" => shell_plan(input, Platform::Linux),
        "shell_powershell" => shell_plan(input, Platform::PowerShell),
        "run_task" => task_plan(input),
        "run_verification_command" => verification_plan(input),
        "git_diff_review" => crate::git_diff_plan::plan(input),
        _ => Err(tool_err(
            "unknown_tool",
            format!("未知静态命令工具：{tool}"),
        )),
    }
}

fn shell_plan(input: &Value, platform: Platform) -> Result<CommandPlan, ToolError> {
    let obj = object(input)?;
    reject_unknown(obj, &["command", "timeout_secs", "max_output_bytes"])?;
    let command = command(obj, "command")?;
    reject_likely_mutation(&command)?;
    Ok(CommandPlan {
        command,
        timeout_secs: bounded_int(
            obj,
            "timeout_secs",
            SHELL_TIMEOUT_DEFAULT,
            1,
            SHELL_TIMEOUT_MAX,
        )?,
        max_output_bytes: bounded_output(obj, SHELL_OUTPUT_DEFAULT, SHELL_OUTPUT_MAX)?,
        platform,
    })
}

fn task_plan(input: &Value) -> Result<CommandPlan, ToolError> {
    let obj = object(input)?;
    reject_unknown(obj, &["kind", "timeout_secs", "max_output_bytes"])?;
    let kind = command(obj, "kind")?;
    let command = match kind.as_str() {
        "test" => "cargo test",
        "build" => "cargo build",
        "lint" => "cargo clippy -- -D warnings",
        "typecheck" | "cargo_check" => "cargo check",
        _ => {
            return Err(tool_err(
                "bad_input",
                "kind 必须是 test、build、lint、typecheck 或 cargo_check",
            ));
        }
    };
    Ok(CommandPlan {
        command: command.into(),
        timeout_secs: bounded_int(
            obj,
            "timeout_secs",
            TASK_TIMEOUT_DEFAULT,
            1,
            TASK_TIMEOUT_MAX,
        )?,
        max_output_bytes: bounded_output(obj, TASK_OUTPUT_DEFAULT, TASK_OUTPUT_MAX)?,
        platform: Platform::PosixHost,
    })
}

fn verification_plan(input: &Value) -> Result<CommandPlan, ToolError> {
    let obj = object(input)?;
    reject_unknown(obj, &["command"])?;
    let command = command(obj, "command")?;
    reject_likely_mutation(&command)?;
    Ok(CommandPlan {
        command,
        timeout_secs: VERIFY_TIMEOUT_SECS,
        max_output_bytes: VERIFY_OUTPUT_MAX,
        platform: Platform::PosixHost,
    })
}

pub(crate) fn object(input: &Value) -> Result<&Map<String, Value>, ToolError> {
    input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "输入必须是对象"))
}

pub(crate) fn reject_unknown(obj: &Map<String, Value>, allowed: &[&str]) -> Result<(), ToolError> {
    if let Some(field) = obj.keys().find(|field| !allowed.contains(&field.as_str())) {
        return Err(tool_err("bad_input", format!("不支持的字段：{field}")));
    }
    Ok(())
}

fn command(obj: &Map<String, Value>, field: &str) -> Result<String, ToolError> {
    let value = obj
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| tool_err("bad_input", format!("{field} 是必填字符串")))?
        .trim();
    if value.is_empty() || value.len() > 32_768 || value.contains('\0') {
        return Err(tool_err(
            "bad_input",
            format!("{field} 必须是 1..=32768 字节的非空文本"),
        ));
    }
    Ok(value.to_owned())
}

fn bounded_int(
    obj: &Map<String, Value>,
    field: &str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ToolError> {
    match obj.get(field) {
        None => Ok(default),
        Some(value) => {
            let value = value
                .as_u64()
                .ok_or_else(|| tool_err("bad_input", format!("{field} 必须是整数")))?;
            if !(min..=max).contains(&value) {
                return Err(tool_err(
                    "bad_input",
                    format!("{field} 必须在 {min}..={max} 之间"),
                ));
            }
            Ok(value)
        }
    }
}

pub(crate) fn bounded_output(
    obj: &Map<String, Value>,
    default: usize,
    max: usize,
) -> Result<usize, ToolError> {
    usize::try_from(bounded_int(
        obj,
        "max_output_bytes",
        default as u64,
        256,
        max as u64,
    )?)
    .map_err(|_| tool_err("bad_input", "max_output_bytes 超出本机 usize 范围"))
}

pub(crate) fn optional_bool(
    obj: &Map<String, Value>,
    field: &str,
    default: bool,
) -> Result<bool, ToolError> {
    match obj.get(field) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| tool_err("bad_input", format!("{field} 必须是布尔值"))),
    }
}

fn reject_likely_mutation(command: &str) -> Result<(), ToolError> {
    let lowercase = command.to_ascii_lowercase();
    let words: Vec<&str> = lowercase
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty())
        .collect();
    let unsafe_fragment = if lowercase.contains('>') {
        Some(">")
    } else if let Some(word) = words.iter().copied().find(|word| {
        matches!(
            *word,
            "rm" | "mv"
                | "cp"
                | "touch"
                | "mkdir"
                | "rmdir"
                | "chmod"
                | "chown"
                | "tee"
                | "truncate"
        )
    }) {
        Some(word)
    } else if words.windows(2).any(|pair| {
        matches!(
            pair,
            [
                "git",
                "add" | "commit" | "reset" | "clean" | "restore" | "checkout" | "switch"
            ] | ["cargo", "fmt"]
                | ["npm" | "pnpm", "install"]
        )
    }) {
        Some("known write command")
    } else {
        None
    };
    match unsafe_fragment {
        Some(fragment) => Err(tool_err(
            "mutation_not_allowed",
            format!("验证和静态 shell 工具不允许可能修改 workspace 的命令片段：{fragment}"),
        )),
        None => Ok(()),
    }
}
