//! 独立测试 agent（issue 027）验收点 3：
//! “kill -9 进程 → 重启 → 会话还在、能接着聊、undo 栈还能用”。
//!
//! 会话 A：两轮 + `/quit`（正常退出，不需要真的 kill -9 才能验证“重启续会”
//! 这条链路——`Session::restore` 走的是同一条载入路径，真正的 `kill -9`
//! 端到端场景由 `indep_unresolved_tool_recovery.rs` 覆盖）→ 同一个
//! `--session <path>` 起会话 B → 能接着问（第 3 轮请求体含前两轮上下文）→
//! `/undo` 在 B 里仍工作（undo 栈是从盘上重建的，不是内存里带过来的）。

mod indep_support;

use indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[test]
fn restarting_the_same_session_path_continues_the_conversation_and_undo_still_works() {
    let scratch = Scratch::new("restart-continue");
    let session = scratch.path("s.jsonl");

    // 会话 A：两轮 + /quit。
    {
        let server = FakeServer::start(vec![
            Script::Immediate(sse::text_reply("turn one reply")),
            Script::Immediate(sse::text_reply("turn two reply")),
        ]);
        let providers = scratch.write_providers_toml(&server.base_url());
        let mut cli = CliProcess::spawn(&providers, Some(&session));
        assert!(
            cli.wait_for("输入一句话开始对话", T),
            "会话 A 启动横幅超时：{}",
            cli.combined_output()
        );

        cli.send_line("alpha message");
        assert!(
            cli.wait_for("[本轮完成]", T),
            "会话 A 第一轮没完成：{}",
            cli.combined_output()
        );
        cli.send_line("beta message");
        assert!(
            cli.wait_for("[本轮完成]", T),
            "会话 A 第二轮没完成：{}",
            cli.combined_output()
        );
        cli.send_line("/quit");
        assert!(
            cli.wait_exit(T).is_some(),
            "会话 A 该干净退出：{}",
            cli.combined_output()
        );
        assert_eq!(server.bodies().len(), 2, "会话 A 该恰好发生两次网络请求");
    }

    // 会话 B：同一路径重开，能接着问——第 3 个网络请求（会话 B 的第 1 个）
    // 的请求体该含会话 A 两轮的完整上下文。
    let server_b = FakeServer::start(vec![Script::Immediate(sse::text_reply("gamma reply"))]);
    let providers_b = scratch.write_providers_toml(&server_b.base_url());
    let mut cli_b = CliProcess::spawn(&providers_b, Some(&session));
    assert!(
        cli_b.wait_for("[会话已恢复]", T),
        "该看到恢复横幅：{}",
        cli_b.combined_output()
    );

    cli_b.send_line("gamma message");
    assert!(
        cli_b.wait_for("[本轮完成]", T),
        "会话 B 新一轮没完成：{}",
        cli_b.combined_output()
    );

    let bodies_b = server_b.bodies();
    assert_eq!(bodies_b.len(), 1, "会话 B 该恰好发生一次网络请求");
    let req3 = &bodies_b[0];
    assert!(
        req3.contains("alpha message"),
        "该带上会话 A 第一轮的输入：{req3}"
    );
    assert!(
        req3.contains("turn one reply"),
        "该带上会话 A 第一轮的回复：{req3}"
    );
    assert!(
        req3.contains("beta message"),
        "该带上会话 A 第二轮的输入：{req3}"
    );
    assert!(
        req3.contains("turn two reply"),
        "该带上会话 A 第二轮的回复：{req3}"
    );
    assert!(req3.contains("gamma message"), "该带上新一轮的输入：{req3}");

    // /undo 在 B 里仍工作——undo 栈是从盘上重建的。
    cli_b.send_line("/undo");
    assert!(
        cli_b.wait_for("[已撤销]", T),
        "undo 栈该从磁盘恢复：{}",
        cli_b.combined_output()
    );
    assert!(
        cli_b.stdout_snapshot().contains("第 3 轮"),
        "该退掉刚才在 B 里新开的第 3 轮"
    );

    cli_b.send_line("/quit");
    assert!(
        cli_b.wait_exit(T).is_some(),
        "会话 B 该干净退出：{}",
        cli_b.combined_output()
    );
}
