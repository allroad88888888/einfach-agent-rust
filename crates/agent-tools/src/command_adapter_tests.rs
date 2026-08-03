use super::*;
use crate::command_plan::{Platform, plan};
use serde_json::json;
use std::path::Path;

#[test]
fn recognizes_exactly_the_six_static_command_names() {
    for name in [
        "shell_macos",
        "shell_linux",
        "shell_powershell",
        "run_task",
        "run_verification_command",
        "git_diff_review",
    ] {
        assert!(is_static_command_tool(name));
    }
    assert!(!is_static_command_tool("srv:shell/exec"));
    assert!(!is_static_command_tool("shell_linux "));
}

#[test]
fn shell_commands_are_closed_and_reject_likely_workspace_mutation() {
    let err = plan(
        "shell_linux",
        &json!({ "command": "printf ok", "cwd": "." }),
    )
    .unwrap_err();
    assert_eq!(&*err.code, "bad_input");

    let err = plan("shell_linux", &json!({ "command": "touch changed.txt" })).unwrap_err();
    assert_eq!(&*err.code, "mutation_not_allowed");

    let err = plan(
        "run_verification_command",
        &json!({ "command": "sh -c 'rm changed.txt'" }),
    )
    .unwrap_err();
    assert_eq!(&*err.code, "mutation_not_allowed");
}

#[test]
fn task_aliases_map_to_fixed_cargo_commands() {
    assert_eq!(
        plan("run_task", &json!({ "kind": "test" }))
            .unwrap()
            .command,
        "cargo test"
    );
    assert_eq!(
        plan("run_task", &json!({ "kind": "build" }))
            .unwrap()
            .command,
        "cargo build"
    );
    assert_eq!(
        plan("run_task", &json!({ "kind": "lint" }))
            .unwrap()
            .command,
        "cargo clippy -- -D warnings"
    );
    assert_eq!(
        plan("run_task", &json!({ "kind": "typecheck" }))
            .unwrap()
            .command,
        "cargo check"
    );
    assert_eq!(
        plan("run_task", &json!({ "kind": "cargo_check" }))
            .unwrap()
            .command,
        "cargo check"
    );
}

#[test]
fn verification_uses_fixed_timeout_and_output_budget() {
    let plan = plan(
        "run_verification_command",
        &json!({ "command": "cargo check" }),
    )
    .unwrap();
    assert_eq!(plan.timeout_secs, 300);
    assert_eq!(plan.max_output_bytes, 131_072);
}

#[test]
fn git_diff_quotes_paths_and_rejects_traversal_or_ref_injection() {
    let review = plan(
        "git_diff_review",
        &json!({ "base": "HEAD~1", "paths": ["src/it's.rs"], "include_stat": true }),
    )
    .unwrap();
    assert!(
        review
            .command
            .contains("diff --no-ext-diff --stat 'HEAD~1' -- 'src/it'\"'\"'s.rs'")
    );
    assert!(
        review
            .command
            .contains("; git -c core.pager=cat diff --no-ext-diff 'HEAD~1' --")
    );

    let traversal = plan("git_diff_review", &json!({ "paths": ["../secret"] })).unwrap_err();
    assert_eq!(&*traversal.code, "bad_input");
    let injection = plan(
        "git_diff_review",
        &json!({ "base": "HEAD --output=/tmp/x" }),
    )
    .unwrap_err();
    assert_eq!(&*injection.code, "bad_input");
}

#[test]
fn host_platform_is_checked_before_spawning_a_shell() {
    let plan = plan(
        "shell_powershell",
        &json!({ "command": "Write-Output proof" }),
    )
    .unwrap();
    assert_eq!(plan.platform, Platform::PowerShell);
    let err = ensure_platform(plan.platform).unwrap_err();
    assert_eq!(&*err.code, "unsupported_platform");
}

#[cfg(unix)]
#[test]
fn verification_adapter_reuses_shell_executor_for_noninteractive_evidence() {
    let output = execute(
        Path::new(env!("CARGO_MANIFEST_DIR")),
        "run_verification_command",
        &json!({ "command": "printf static-command-proof" }),
    )
    .unwrap();
    assert_eq!(output, "static-command-proof");
}
