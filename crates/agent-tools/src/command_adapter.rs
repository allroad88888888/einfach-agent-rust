//! 静态命令工具的受限执行。
//!
//! 输入解析在 `command_plan.rs`；本模块只复用既有 `shell::execute` 并限制响应。

use crate::ToolError;
use crate::command_plan::{Platform, plan};
use crate::exec::tool_err;
use crate::shell;
use serde_json::{Value, json};
use std::path::Path;

const TRUNCATION_SUFFIX: &str = "\n[truncated: narrow the command or paths]\n";

/// 静态命令名由主 executor 分发到这里；其余名字仍由既有 `exec.rs` 处理。
pub(crate) fn is_static_command_tool(tool: &str) -> bool {
    matches!(
        tool,
        "shell_macos"
            | "shell_linux"
            | "shell_powershell"
            | "run_task"
            | "run_verification_command"
            | "git_diff_review"
    )
}

/// 解析、执行并裁剪输出。平台不匹配时在 spawn 前返回 `unsupported_platform`。
pub(crate) fn execute(root: &Path, tool: &str, input: &Value) -> Result<String, ToolError> {
    let plan = plan(tool, input)?;
    ensure_platform(plan.platform)?;
    let raw = shell::execute(
        root,
        &json!({ "cmd": plan.command, "timeout_secs": plan.timeout_secs }),
    )?;
    Ok(limit_output(&raw, plan.max_output_bytes))
}

fn ensure_platform(platform: Platform) -> Result<(), ToolError> {
    let supported = match platform {
        Platform::Macos => cfg!(target_os = "macos"),
        Platform::Linux => cfg!(target_os = "linux"),
        Platform::PowerShell => false,
        Platform::PosixHost => cfg!(unix),
    };
    if supported {
        Ok(())
    } else {
        Err(tool_err(
            "unsupported_platform",
            "该静态命令工具没有适用于当前宿主的 shell backend",
        ))
    }
}

fn limit_output(output: &str, max_bytes: usize) -> String {
    if output.len() <= max_bytes {
        return output.into();
    }
    let keep = max_bytes.saturating_sub(TRUNCATION_SUFFIX.len());
    let mut boundary = keep;
    while boundary > 0 && !output.is_char_boundary(boundary) {
        boundary -= 1;
    }
    format!("{}{}", &output[..boundary], TRUNCATION_SUFFIX)
}

#[cfg(test)]
#[path = "command_adapter_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "command_adapter_perf_tests.rs"]
mod perf_tests;
