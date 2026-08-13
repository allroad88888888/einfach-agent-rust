//! `shell::execute` 的单测。**从 `shell.rs` 拆出来的**（红线 9：那个文件加了
//! 进程组轮询之后顶破 300 行），照本 crate 既有形状——`apply_patch_spec.rs` /
//! `command_adapter.rs` 都是这么挂 `#[path]` 兄弟文件的。

use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

fn temp_root(name: &str) -> std::path::PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-tools-shell-{name}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn stdout_is_returned_verbatim() {
    let root = temp_root("stdout");
    let out = execute(&root, &json!({"cmd": "echo hello"})).unwrap();
    assert_eq!(out, "hello\n");
}

#[test]
fn nonzero_exit_is_ok_with_exit_code_marker() {
    let root = temp_root("nonzero");
    let out = execute(&root, &json!({"cmd": "exit 3"})).unwrap();
    assert_eq!(out, "\n[exit code: 3]");
}

#[test]
fn stderr_is_appended_after_stdout() {
    let root = temp_root("stderr");
    let out = execute(&root, &json!({"cmd": "echo out; echo err 1>&2"})).unwrap();
    assert_eq!(out, "out\n\n[stderr]\nerr\n");
}

#[test]
fn stderr_and_nonzero_exit_both_appended_in_order() {
    let root = temp_root("both");
    let out = execute(&root, &json!({"cmd": "echo out; echo err 1>&2; exit 5"})).unwrap();
    assert_eq!(out, "out\n\n[stderr]\nerr\n\n[exit code: 5]");
}

#[test]
fn zero_exit_no_stderr_has_no_markers() {
    let root = temp_root("clean");
    let out = execute(&root, &json!({"cmd": "true"})).unwrap();
    assert_eq!(out, "");
}

#[test]
fn cwd_is_locked_to_root() {
    let root = temp_root("cwd");
    std::fs::write(root.join("marker.txt"), "x").unwrap();
    let out = execute(&root, &json!({"cmd": "ls"})).unwrap();
    assert_eq!(out, "marker.txt\n");
}

#[test]
fn timeout_returns_err_promptly() {
    let root = temp_root("timeout");
    let started = std::time::Instant::now();
    let err = execute(&root, &json!({"cmd": "sleep 60", "timeout_secs": 1})).unwrap_err();
    assert_eq!(&*err.code, "timeout");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "超时应该在远小于原命令耗时内返回，实际 {:?}",
        started.elapsed()
    );
}

#[cfg(unix)]
#[test]
fn timeout_kills_the_whole_process_group_no_orphans() {
    let root = temp_root("orphan");
    let marker = root.join("pgid.txt");
    // sh 自己的 pid（`$$`）就是这个进程组的 pgid（`process_group(0)`）。
    // 先落盘再起两个孙进程（一个后台、一个前台），超时后验证整组已死。
    let cmd = format!("echo $$ > {}; sleep 60 & sleep 60", marker.display());

    let err = run(&root, &cmd, 1).unwrap_err();
    assert_eq!(&*err.code, "timeout");

    let pgid_str = std::fs::read_to_string(&marker).unwrap();
    let pgid: i32 = pgid_str.trim().parse().unwrap();

    // signal 0：不发信号，只探测进程组还在不在。返回 -1（ESRCH）才是「杀干净了」。
    //
    // **有界轮询而不是瞬时断言**：「杀干净」是个最终性质，不是瞬时性质。
    // SIGKILL 之后孙进程要先变成僵尸、再等 init 收尸才从进程表里消失，
    // 而僵尸对 `kill(pid, 0)` 是**存在**的（返回 0）。这一步的耗时跟机器负载有关
    // ——本机 macOS 一直是瞬时过的，CI 的 Linux runner 上第一次跑就红了
    // （left: 0, right: -1）。轮询测的仍然是同一件事：不许有孤儿活下来。
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut alive = unsafe { libc::kill(-pgid, 0) };
    while alive != -1 && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        alive = unsafe { libc::kill(-pgid, 0) };
    }
    assert_eq!(alive, -1, "进程组应该已经被杀干净，不该还查得到任何成员");
}

#[test]
fn spawn_failed_for_nonexistent_shell_is_impossible_but_bad_root_input_is_bad_input() {
    // `sh` 本身不存在是没法在这台机器上稳定构造的场景，这里换成验入参：
    // cmd 缺失/类型不对 → bad_input，不会走到 spawn 那一步。
    let root = temp_root("badinput");
    let err = execute(&root, &json!({})).unwrap_err();
    assert_eq!(&*err.code, "bad_input");

    let err = execute(&root, &json!({"cmd": 42})).unwrap_err();
    assert_eq!(&*err.code, "bad_input");

    let err = execute(&root, &json!("not-an-object")).unwrap_err();
    assert_eq!(&*err.code, "bad_input");
}

#[test]
fn timeout_secs_default_is_thirty() {
    let obj = json!({}).as_object().unwrap().clone();
    assert_eq!(parse_timeout(&obj).unwrap(), 30);
}

#[test]
fn timeout_secs_bounds_are_inclusive_1_to_300() {
    let mut obj = Map::new();
    obj.insert("timeout_secs".to_string(), json!(1));
    assert_eq!(parse_timeout(&obj).unwrap(), 1);

    let mut obj = Map::new();
    obj.insert("timeout_secs".to_string(), json!(300));
    assert_eq!(parse_timeout(&obj).unwrap(), 300);

    let mut obj = Map::new();
    obj.insert("timeout_secs".to_string(), json!(0));
    assert_eq!(&*parse_timeout(&obj).unwrap_err().code, "bad_input");

    let mut obj = Map::new();
    obj.insert("timeout_secs".to_string(), json!(301));
    assert_eq!(&*parse_timeout(&obj).unwrap_err().code, "bad_input");
}

#[test]
fn timeout_secs_wrong_type_is_bad_input() {
    let mut obj = Map::new();
    obj.insert("timeout_secs".to_string(), json!("thirty"));
    assert_eq!(&*parse_timeout(&obj).unwrap_err().code, "bad_input");
}
