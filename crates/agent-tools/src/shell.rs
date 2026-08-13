//! `srv:shell/exec`：在 `root` 下跑一条 shell 命令——第一个
//! `Reversibility::Irreversible` 的工具（issue 020）。
//!
//! **监狱语义到这里变了**：`current_dir` 锁在 `root`，这只保证命令**起点**在
//! root 之内——`cmd` 内容本身可以 `cd ..`、可以用绝对路径、可以是
//! `rm -rf /`，Rust 这一层拦不住。`fs/read`/`fs/list` 能把访问范围关进监狱是
//! 因为它们自己解释路径参数；shell 命令是 `/bin/sh` 在解释，我们只决定它从
//! 哪里出发，不决定它去哪。这正是它被判 `Irreversible` 而不是靠更强隔离降级
//! 成 `Reversible` 的原因——docs/TOOLS.md 的判据是「拿不准就 Irreversible」，
//! 而 shell 是「拿得准」的那一类：没有沙箱，就是不可逆，别假装能 jail。
//!
//! **超时与孤儿**：子进程（`sh`）在 Unix 上放进一个新的进程组
//! （`process_group(0)`：pgid 设成它自己的 pid），超时后对**负 pgid** 发
//! `SIGKILL`，一并带走它 fork 出的孙进程（比如 `sh -c 'sleep 60 & sleep 60'`
//! 的两个 `sleep`）——只杀 `sh` 自己会把孙进程留成孤儿，杀组才是干净的。
//! 杀信号之后阻塞等一次后台线程的 `wait_with_output`，确保子进程被真正
//! reap 掉（发信号只是让它死，收尸才是它从进程表消失）。

use crate::ToolError;
use crate::exec::tool_err;
use serde_json::{Map, Value};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 300;

pub(crate) fn execute(root: &Path, input: &Value) -> Result<String, ToolError> {
    let obj = input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "input 必须是对象"))?;

    let cmd = obj
        .get("cmd")
        .and_then(Value::as_str)
        .ok_or_else(|| tool_err("bad_input", "cmd 是必填字符串"))?;

    let timeout_secs = parse_timeout(obj)?;

    run(root, cmd, timeout_secs)
}

/// `timeout_secs`：缺省 30，类型不对或越出 `1..=300` → `bad_input`。这一段是
/// 输入校验，跑在任何进程被 spawn 之前——跟「只有起不来/超时才是 Err」
/// 不矛盾，那句话说的是**执行结果**，不是入参形状。
fn parse_timeout(obj: &Map<String, Value>) -> Result<u64, ToolError> {
    match obj.get("timeout_secs") {
        None | Some(Value::Null) => Ok(DEFAULT_TIMEOUT_SECS),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| tool_err("bad_input", "timeout_secs 必须是整数"))?;
            if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&n) {
                return Err(tool_err(
                    "bad_input",
                    format!("timeout_secs 必须在 {MIN_TIMEOUT_SECS}..={MAX_TIMEOUT_SECS} 之间"),
                ));
            }
            Ok(n)
        }
    }
}

/// spawn → 后台线程等待（带超时）→ 格式化输出。
///
/// 只有 spawn 本身失败或超时是 `Err`：命令跑起来了但退出码非零、或有
/// stderr，仍然是 `Ok`——模型自己看输出决定要不要紧（003 的部分失败语义，
/// executor 不替模型下判断）。
fn run(root: &Path, cmd: &str, timeout_secs: u64) -> Result<String, ToolError> {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(root)
        // 显式关 stdin：不关的话，会读 stdin 的命令在非交互场景下会挂起等
        // 永远不会来的输入，那不是「超时」该管的慢，是另一类挂死。
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pgid = 0 ⇒ 子进程 setpgid(自己, 0)：自己成为新进程组的组长。它 fork
        // 出的孙进程默认继承这个 pgid，超时时杀 -pgid 才能一网打尽。
        command.process_group(0);
    }

    let child = command
        .spawn()
        .map_err(|e| tool_err("spawn_failed", format!("启动失败：{e}")))?;
    let pid = child.id();

    // `wait_with_output` 内部并发读 stdout/stderr 再等退出，不会像手动顺序
    // read 两个管道那样在其中一个写满时死锁。挪进后台线程是为了让主线程能
    // 对它施加超时——std 没有自带「等一个子进程但最多等 N 秒」的 API。
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = child.wait_with_output();
        let _ = tx.send(result);
    });

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(output)) => Ok(format_output(&output)),
        Ok(Err(e)) => Err(tool_err("spawn_failed", format!("等待子进程失败：{e}"))),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_group(pid);
            // 阻塞等后台线程真正把子进程 reap 掉——发 SIGKILL 只是让它死，
            // wait() 收尸才会让它从进程表消失，不等就是留了半秒的僵尸窗口。
            let _ = rx.recv();
            Err(tool_err(
                "timeout",
                format!("命令超时（{timeout_secs}s）：{cmd}"),
            ))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // 后台线程异常退出（理论上不会——它只 wait_with_output 再 send）。
            // 兜底杀一次组，不留潜在孤儿。
            kill_group(pid);
            Err(tool_err("spawn_failed", "等待子进程的后台线程异常退出"))
        }
    }
}

/// stdout 原样打头；stderr 非空则追加 `\n[stderr]\n<内容>`；非成功退出（含
/// 被信号杀死，此时 `status.code()` 是 `None`）追加 `\n[exit code: N]`。
fn format_output(output: &Output) -> String {
    let mut out = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        out.push_str("\n[stderr]\n");
        out.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        out.push_str(&format!("\n[exit code: {code}]"));
    }
    out
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // `pgid == pid`（spawn 时 `process_group(0)` 定的），杀负 pgid 杀整个组。
    // 目标可能已经自然退出——kill 一个不存在的 pid 返回 ESRCH，忽略即可，
    // 不是需要上报的错误。
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {
    // 非 Unix 没有进程组语义（本仓当前只在 Unix 上跑，见 020 的验收原文），
    // 留空占位避免编译失败，不在这条路径上假装做了清理。
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
