//! 148 起这些用例住在 `tool_table_standard` 底下（被测的两档构造器搬去了那里），
//! `super::*` 因此只白拿 `ToolTable`/`ToolSpec`；`snapshot` 那几条要的三样各自
//! 显式 `use`——076 那句「名字规则搬走之后不再白拿 `Location`」的同款延续。
use std::sync::Arc;

use agent_core::{Location, Reversibility};
use serde_json::Value;

use super::*;

#[test]
fn standard_local_exposes_the_safe_local_standard_set_in_stable_order() {
    let table = ToolTable::standard_local();
    let names: Vec<&str> = table.specs().iter().map(|spec| &*spec.name).collect();
    assert_eq!(
        names,
        vec![
            "read_file",
            "list_files",
            "search_files",
            "rg_search",
            "apply_patch",
            "write_file",
            "delete_path",
            "copy_path",
            "move_path",
            "revert_workspace_change",
            "find_test_lint_commands",
            "shell_macos",
            "shell_linux",
            "shell_powershell",
            "run_task",
            "run_verification_command",
            "git_diff_review",
        ]
    );
}

#[test]
fn standard_local_reversibility_only_marks_verified_reads_as_pure() {
    let table = ToolTable::standard_local();
    for tool in [
        "read_file",
        "list_files",
        "search_files",
        "rg_search",
        "find_test_lint_commands",
        "git_diff_review",
    ] {
        assert_eq!(
            table.snapshot(tool, Arc::new(Value::Null)).reversibility,
            Reversibility::Pure,
            "{tool}"
        );
    }
    for tool in [
        "apply_patch",
        "write_file",
        "delete_path",
        "copy_path",
        "move_path",
        "revert_workspace_change",
        "shell_linux",
        "run_task",
        "run_verification_command",
    ] {
        assert_eq!(
            table.snapshot(tool, Arc::new(Value::Null)).reversibility,
            Reversibility::Irreversible,
            "{tool}"
        );
    }
}

#[test]
fn standard_adds_only_the_three_web_interaction_tools() {
    let table = ToolTable::standard();
    let names: Vec<&str> = table.specs().iter().map(|spec| &*spec.name).collect();
    assert_eq!(
        &names[17..],
        ["ask_user_question", "browser_action", "save_file"]
    );
    for tool in ["ask_user_question", "browser_action", "save_file"] {
        let snapshot = table.snapshot(tool, Arc::new(Value::Null));
        assert_eq!(snapshot.location, Location::Web, "{tool}");
    }
    assert_eq!(
        table
            .snapshot("ask_user_question", Arc::new(Value::Null))
            .reversibility,
        Reversibility::Pure
    );
    assert_eq!(
        table
            .snapshot("browser_action", Arc::new(Value::Null))
            .reversibility,
        Reversibility::Irreversible
    );
    assert_eq!(
        table
            .snapshot("save_file", Arc::new(Value::Null))
            .reversibility,
        Reversibility::Irreversible
    );
    assert!(
        !names
            .iter()
            .any(|name| name.starts_with("srv:agent/") || name.starts_with("srv:mcp/"))
    );
}
