//! 独立测试 agent（issue 027）验收点 6：
//! “恢复时未收敛槽——用会话 A 真跑到 ToolsPending 再 kill 子进程（SIGKILL）
//! → 重启 → 输出含‘可能已经执行’且假服务器没有收到该工具的重发”。
//!
//! 模型发起一次 `srv:shell/exec`（cmd 是 `sleep 2`，给测试留出下手的窗口），
//! 在工具还没跑完、结果还没落地时对整个 CLI 进程 SIGKILL；`ToolSlot` 落盘
//! 时永远停在 `Pending`，没有收敛的那条 Entry。重启后 `main.rs` 该走
//! `has_unresolved_tool_calls` 分支：提示“可能已经执行过”，**不自动重发**
//! ——既不重新执行本地工具，也不把这个未完成的工具调用回执发回 provider。

mod indep_support;

use indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::time::Duration;

const T: Duration = Duration::from_secs(15);

#[test]
fn sigkill_during_a_pending_tool_call_is_recovered_as_maybe_already_executed_and_never_resent() {
    let scratch = Scratch::new("unresolved-tool");
    let session = scratch.path("s.jsonl");

    {
        // 会话 A：模型发起一次 srv:shell/exec（睡 2 秒），在它还没跑完时
        // SIGKILL 整个 CLI 进程——ToolSlot 落盘时是 Pending，永远等不到
        // 收敛的那条 Entry。
        let server = FakeServer::start(vec![Script::Immediate(sse::tool_call(
            "call_pending",
            sse::SHELL_EXEC_WIRE_NAME,
            r#"{"cmd": "sleep 2"}"#,
        ))]);
        let providers = scratch.write_providers_toml(&server.base_url());
        let mut cli = CliProcess::spawn(&providers, Some(&session));
        assert!(cli.wait_for("输入一句话开始对话", T), "会话 A 启动横幅超时：{}", cli.combined_output());

        cli.send_line("please run a slow command");
        // 等工具真的开始跑（看到派发日志），给 fire-and-forget 落盘留出余量，
        // 再在它跑完（睡 2 秒）之前动手，保证 ToolSlot 落盘时还停在 Pending。
        assert!(cli.wait_for("[tool] srv:shell/exec", T), "该看到工具已经派发：{}", cli.combined_output());
        std::thread::sleep(Duration::from_millis(700));
        cli.send_signal("-9");
        assert!(cli.wait_exit(T).is_some(), "SIGKILL 之后进程该确实终止");
    }

    // 重启：输出该含“可能已经执行过”的提示，且假服务器一次请求都不该
    // 收到——不自动重发工具调用，也不自动再问一次 provider。
    let server_b = FakeServer::start(vec![]);
    let providers_b = scratch.write_providers_toml(&server_b.base_url());
    let mut cli_b = CliProcess::spawn(&providers_b, Some(&session));

    assert!(cli_b.wait_for("[会话已恢复]", T), "该能恢复会话：{}", cli_b.combined_output());
    assert!(cli_b.wait_for("可能已经执行过", T), "该提示可能已经执行过：{}", cli_b.combined_output());
    let out = cli_b.combined_output();
    assert!(out.contains("不会自动重发") || out.contains("不重发"), "该明确说明不会自动重发：{out}");

    cli_b.send_line("/quit");
    assert!(cli_b.wait_exit(T).is_some(), "该干净退出：{}", cli_b.combined_output());

    assert_eq!(server_b.request_count(), 0, "不该重发那个未收敛的工具调用：{:?}", server_b.bodies());
}
