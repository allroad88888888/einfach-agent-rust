//! `srv:shell/exec` 超时孤儿清理（issue 020 验收「超时能中断子进程，不留孤儿」）：
//! 子进程用 `&` 甩出去一个孙进程，超时后杀的必须是**整个进程组**，不能只杀直接
//! 子进程——否则孙进程会脱钩变成真正的孤儿，在系统里继续跑到自然结束。
//!
//! 用一个不太可能与别的用例/系统进程撞车的 sleep 秒数；测试收尾无论断言是否
//! 通过都尝试 `pkill` 兜底，避免脏环境。

mod support;

use std::process::Command;
use std::time::{Duration, Instant};

use agent_tools::ToolExecutor;
use serde_json::json;
use support::TestRoot;

// 独特秒数：91。不撞其余用例用到的 sleep 1/60，也不太可能撞其他系统进程。
const MARK: &str = "sleep 91";

/// 测试收尾兜底：无论断言是否 panic，`Drop` 都会跑到，尽力清场。
struct KillGuard;
impl Drop for KillGuard {
    fn drop(&mut self) {
        let _ = Command::new("pkill").arg("-f").arg(MARK).status();
    }
}

fn count_matching_processes() -> usize {
    let out = Command::new("pgrep")
        .arg("-f")
        .arg(MARK)
        .output()
        .expect("pgrep 必须可执行（测试环境依赖）");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

#[test]
fn timeout_kills_the_whole_process_group_no_orphaned_grandchild() {
    let _guard = KillGuard;
    let root = TestRoot::new("shell-orphan");
    let exec = ToolExecutor::new(root.path()).unwrap();

    // 子进程（sh -c 起的第一个 sleep）把第二个 sleep 用 `&` 甩到后台，变成孙进程。
    // 只杀直接子进程留不住孙进程；必须杀整个进程组。
    let err = exec
        .execute(
            "srv:shell/exec",
            &json!({ "cmd": format!("{MARK} & {MARK}"), "timeout_secs": 1 }),
        )
        .expect_err("超时必须是 Err");
    assert_eq!(err.code.as_ref(), "timeout");

    // 杀进程组是异步生效的（SIGKILL 送出到进程表更新之间有极短窗口），轮询给够
    // 余量；但不应该接近 91 秒的自然结束时间，那就说明孙进程根本没被杀，只是
    // 巧合还没到期。
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut remaining = count_matching_processes();
    while remaining > 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        remaining = count_matching_processes();
    }

    assert_eq!(
        remaining, 0,
        "超时后必须不留孤儿：pgrep -f {MARK:?} 应该为空，实际还有 {remaining} 个进程存活"
    );
}
