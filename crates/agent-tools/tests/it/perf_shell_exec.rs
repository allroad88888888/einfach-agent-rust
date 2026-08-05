//! `srv:shell/exec` 的确定性输出预算：一条命令只执行一次，输出行数可预测，并在
//! core 边界截断到 prompt 上限。进程数目前没有公开计数器，因此用 root 内的单次
//! 命令标记验证一次 `execute` 只触发一次命令体；不使用 wall-clock 断言。

use agent_tools::ToolExecutor;
use serde_json::json;
use crate::support::TestRoot;

const OUTPUT_LINES: usize = 2_048;

#[test]
fn shell_exec_runs_the_command_once_and_has_a_bounded_prompt_view() {
    let root = TestRoot::new("perf-shell-output");
    let exec = ToolExecutor::new(root.path()).unwrap();
    let cmd = r#"
        printf x > invocation-count
        i=0
        while [ "$i" -lt 2048 ]; do
            printf 'line-%04d-xxxxxxxxxxxxxxxx\n' "$i"
            i=$((i + 1))
        done
    "#;

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": cmd }))
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.path().join("invocation-count")).unwrap(),
        "x",
        "一次 execute 只能运行一次命令体"
    );
    assert_eq!(out.lines().count(), OUTPUT_LINES);
    assert!(out.starts_with("line-0000-xxxxxxxxxxxxxxxx\n"));
    assert!(out.ends_with("line-2047-xxxxxxxxxxxxxxxx\n"));
    assert!(out.len() > agent_core::DEFAULT_TOOL_OUTPUT_BYTES);

    let prompt_view = agent_core::truncate_tool_output(&out, agent_core::DEFAULT_TOOL_OUTPUT_BYTES);
    assert!(prompt_view.len() < agent_core::DEFAULT_TOOL_OUTPUT_BYTES + 200);
    assert!(prompt_view.contains("输出被截断"));
}
