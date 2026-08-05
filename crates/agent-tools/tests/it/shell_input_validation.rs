//! `srv:shell/exec` 输入校验（issue 020 钉死规格）：`cmd` 缺失、`timeout_secs`
//! 越界（钉死范围 `1..=300`）都必须是 `bad_input`，不能真的去 spawn 一个空/异常
//! 的进程再失败——错误必须在 schema 校验这一层就截住。

use agent_tools::ToolExecutor;
use serde_json::json;
use crate::support::TestRoot;

#[test]
fn missing_cmd_is_bad_input() {
    let root = TestRoot::new("shell-missing-cmd");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute("srv:shell/exec", &json!({}))
        .expect_err("cmd 缺失必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn timeout_secs_zero_is_below_the_pinned_range_bad_input() {
    let root = TestRoot::new("shell-timeout-zero");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute(
            "srv:shell/exec",
            &json!({ "cmd": "echo hi", "timeout_secs": 0 }),
        )
        .expect_err("timeout_secs=0 越界（钉死范围 1..=300），必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}

#[test]
fn timeout_secs_400_is_above_the_pinned_range_bad_input() {
    let root = TestRoot::new("shell-timeout-400");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let err = exec
        .execute(
            "srv:shell/exec",
            &json!({ "cmd": "echo hi", "timeout_secs": 400 }),
        )
        .expect_err("timeout_secs=400 越界（钉死范围 1..=300），必须是 Err");
    assert_eq!(err.code.as_ref(), "bad_input");
}
