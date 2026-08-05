//! `srv:shell/exec` 输出编码（issue 020 钉死规格）：非零退出码追加
//! `[exit code: N]`；非空 stderr 追加 `[stderr]` 区块。两种都是 `Ok`——只有
//! spawn_failed/timeout 才是 `Err`，退出码非零、有 stderr 都是「模型该看见的
//! 信息」，不是「工具执行失败」。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn nonzero_exit_code_is_ok_with_exit_code_marker() {
    let root = TestRoot::new("shell-exit-3");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": "exit 3" }))
        .expect("非零退出码不是执行失败，必须是 Ok");
    assert!(
        out.contains("[exit code: 3]"),
        "输出必须追加退出码标记，实际输出：{out:?}"
    );
}

#[test]
fn stderr_is_appended_as_a_separate_marked_block() {
    let root = TestRoot::new("shell-stderr");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": "echo err >&2" }))
        .expect("写 stderr 不是执行失败，必须是 Ok");
    assert!(
        out.contains("[stderr]"),
        "必须有 [stderr] 标记，实际输出：{out:?}"
    );
    assert!(
        out.contains("err"),
        "stderr 的原文必须出现在输出里，实际输出：{out:?}"
    );
}

#[test]
fn exit_code_marker_comes_after_stderr_marker_when_both_present() {
    // 钉死规格的顺序：stdout + (非空 stderr → `\n[stderr]\n…`) + (非零退出 →
    // `\n[exit code: N]`)。两个标记都出现时，[stderr] 必须排在 [exit code] 之前。
    let root = TestRoot::new("shell-stderr-and-exit");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": "echo err >&2; exit 7" }))
        .expect("stderr + 非零退出同时出现，依然是 Ok");
    let stderr_pos = out.find("[stderr]").expect("必须含 [stderr]");
    let exit_pos = out.find("[exit code: 7]").expect("必须含 [exit code: 7]");
    assert!(
        stderr_pos < exit_pos,
        "顺序必须是 stdout, [stderr], [exit code]，实际输出：{out:?}"
    );
}
