//! `srv:shell/exec` 超时（issue 020 验收「超时能中断子进程」的前半）：
//! `timeout_secs` 到点后必须在自己设的时限附近返回 `Err`，`code == "timeout"`，
//! **不能**傻等到子进程自己跑完（这里子进程是 `sleep 60`）。

mod support;

use std::time::Instant;

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

#[test]
fn sleep_beyond_timeout_secs_is_err_timeout_within_a_few_seconds() {
    let root = TestRoot::new("shell-timeout");
    let exec = ToolExecutor::new(root.path()).unwrap();

    let started = Instant::now();
    let err = exec
        .execute(
            "srv:shell/exec",
            &json!({ "cmd": "sleep 60", "timeout_secs": 1 }),
        )
        .expect_err("跑满 timeout_secs 必须是 Err，不能等 sleep 60 自己结束");
    let elapsed = started.elapsed();

    assert_eq!(err.code.as_ref(), "timeout");
    assert!(
        elapsed.as_secs_f64() < 5.0,
        "必须在 timeout_secs=1 附近返回，给足调度抖动的宽容度（<5s）；实际用时 {elapsed:?}"
    );
}
