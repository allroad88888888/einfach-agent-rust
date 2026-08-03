//! `git_diff_review` 的只读 Git argv 计划。

use crate::ToolError;
use crate::command_plan::{
    CommandPlan, Platform, bounded_output, object, optional_bool, reject_unknown,
};
use crate::exec::tool_err;
use serde_json::{Map, Value};

const TIMEOUT_SECS: u64 = 30;
const OUTPUT_DEFAULT: usize = 24_576;
const OUTPUT_MAX: usize = 131_072;

pub(crate) fn plan(input: &Value) -> Result<CommandPlan, ToolError> {
    let obj = object(input)?;
    reject_unknown(
        obj,
        &[
            "paths",
            "staged",
            "base",
            "include_stat",
            "max_output_bytes",
        ],
    )?;
    let staged = optional_bool(obj, "staged", false)?;
    let include_stat = optional_bool(obj, "include_stat", true)?;
    let max_output_bytes = bounded_output(obj, OUTPUT_DEFAULT, OUTPUT_MAX)?;
    let base = optional_base(obj)?;
    let paths = optional_paths(obj)?;
    let diff = diff_command(staged, base.as_deref(), &paths, false);
    let command = if include_stat {
        format!(
            "git status --short; {}; {diff}",
            diff_command(staged, base.as_deref(), &paths, true)
        )
    } else {
        format!("git status --short; {diff}")
    };
    Ok(CommandPlan {
        command,
        timeout_secs: TIMEOUT_SECS,
        max_output_bytes,
        platform: Platform::PosixHost,
    })
}

fn optional_base(obj: &Map<String, Value>) -> Result<Option<String>, ToolError> {
    let Some(base) = obj.get("base") else {
        return Ok(None);
    };
    let base = base
        .as_str()
        .ok_or_else(|| tool_err("bad_input", "base 必须是字符串"))?
        .trim();
    if base.is_empty()
        || base.len() > 255
        || base.starts_with('-')
        || base.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(tool_err(
            "bad_input",
            "base 必须是不含空白、控制字符或前导 - 的 Git ref",
        ));
    }
    Ok(Some(base.into()))
}

fn optional_paths(obj: &Map<String, Value>) -> Result<Vec<String>, ToolError> {
    let Some(paths) = obj.get("paths") else {
        return Ok(Vec::new());
    };
    let paths = paths
        .as_array()
        .ok_or_else(|| tool_err("bad_input", "paths 必须是字符串数组"))?;
    if paths.len() > 100 {
        return Err(tool_err("bad_input", "paths 最多 100 项"));
    }
    paths.iter().map(valid_workspace_path).collect()
}

fn valid_workspace_path(value: &Value) -> Result<String, ToolError> {
    let path = value
        .as_str()
        .ok_or_else(|| tool_err("bad_input", "paths 必须是字符串数组"))?
        .trim();
    let absolute =
        path.starts_with('/') || path.starts_with('\\') || path.as_bytes().get(1) == Some(&b':');
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || absolute
        || path.split(['/', '\\']).any(|segment| segment == "..")
    {
        return Err(tool_err(
            "bad_input",
            "paths 必须是 workspace 相对路径，不能包含 ..",
        ));
    }
    Ok(path.into())
}

fn diff_command(staged: bool, base: Option<&str>, paths: &[String], with_stat: bool) -> String {
    let mut command = String::from("git -c core.pager=cat diff --no-ext-diff");
    if staged {
        command.push_str(" --cached");
    }
    if with_stat {
        command.push_str(" --stat");
    }
    if let Some(base) = base {
        command.push(' ');
        command.push_str(&shell_quote(base));
    }
    command.push_str(" --");
    for path in paths {
        command.push(' ');
        command.push_str(&shell_quote(path));
    }
    command
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
