use super::*;
use crate::command_plan::plan;
use serde_json::{Value, json};

fn assert_output_budget(tool: &str, input: Value) {
    let plan = plan(tool, &input).unwrap();
    let oversized = "界".repeat((plan.max_output_bytes / 3) + 100);
    let output = limit_output(&oversized, plan.max_output_bytes);
    assert!(output.len() <= plan.max_output_bytes, "{tool}");
    assert!(output.ends_with(TRUNCATION_SUFFIX), "{tool}");
    assert!(output.is_char_boundary(output.len()), "{tool}");
}

#[test]
fn shell_macos_output_budget_is_deterministic() {
    assert_output_budget("shell_macos", json!({ "command": "printf proof" }));
}

#[test]
fn shell_linux_output_budget_is_deterministic() {
    assert_output_budget("shell_linux", json!({ "command": "printf proof" }));
}

#[test]
fn shell_powershell_output_budget_is_deterministic() {
    assert_output_budget(
        "shell_powershell",
        json!({ "command": "Write-Output proof" }),
    );
}

#[test]
fn run_task_output_budget_is_deterministic() {
    assert_output_budget("run_task", json!({ "kind": "cargo_check" }));
}

#[test]
fn verification_command_output_budget_is_deterministic() {
    assert_output_budget(
        "run_verification_command",
        json!({ "command": "cargo check" }),
    );
}

#[test]
fn git_diff_review_output_budget_is_deterministic() {
    assert_output_budget("git_diff_review", json!({}));
}
