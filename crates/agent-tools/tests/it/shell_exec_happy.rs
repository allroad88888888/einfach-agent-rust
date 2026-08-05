//! `srv:shell/exec` 正常执行（issue 020 验收 1）：`echo hi` 原样回显；`pwd`
//! 证明工作目录锁在 executor 的 root 之内，不是继承调用方进程的 cwd。
//!
//! 钉死规格：`sh -c`，cwd = root；stdout 无 stderr、退出码 0 时原样返回，不追加
//! 任何标记。

mod support;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn echo_hi_returns_stdout_verbatim() {
    let root = TestRoot::new("shell-echo-hi");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": "echo hi" }))
        .expect("echo hi 是最平凡的成功路径，必须是 Ok");
    assert_eq!(
        out, "hi\n",
        "无 stderr、退出码 0 时输出必须是原始 stdout，不追加任何标记"
    );
}

#[test]
fn pwd_reports_the_executor_root_not_the_caller_cwd() {
    let root = TestRoot::new("shell-pwd");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let out = exec
        .execute("srv:shell/exec", &json!({ "cmd": "pwd" }))
        .expect("pwd 不应该失败");

    // `ToolExecutor::new` 的文档：root 会被 canonicalize——子进程的 cwd 是那个
    // canonicalize 之后的路径，不是 TestRoot 给的原始路径（在 macOS 上
    // `std::env::temp_dir()` 经常带一层 `/var` -> `/private/var` 的 symlink，
    // 两边都要走同一次 canonicalize 才能公平比较）。
    let canonical_root =
        std::fs::canonicalize(root.path()).expect("root 必须存在且可 canonicalize");
    let reported = out.trim_end_matches('\n');
    assert_eq!(
        reported,
        canonical_root.to_str().unwrap(),
        "cwd 必须锁在 executor 的 root，不是测试进程自己的 cwd —— 这是「工作目录\
         锁在仓库内」的直接证据"
    );
}
