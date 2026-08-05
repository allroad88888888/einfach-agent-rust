//! 独立测试 agent（issue 027）验收点 2：
//! “/undo 越过一次 shell/exec → 停下推 undo_blocked，/undo! 才越过”。
//!
//! 一轮让模型调 `srv:shell/exec`（脚本回 tool_calls，cmd 用 `echo ok`）→
//! 完成后 `/undo` → 输出含撤销受阻语义与工具名、且该轮没被回退（/redo 能把
//! 刚才被局部撤销的部分找回来，证明状态没丢）→ `/undo!` → 越过成功，且
//! shell 命令全程只被真正执行了一次（undo/redo/undo! 都是纯粹的原子回滚/
//! 重放，不会重新跑一次工具）。

mod indep_support;

use indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[test]
fn undo_stops_at_a_shell_exec_barrier_and_undo_force_crosses_it() {
    let scratch = Scratch::new("shell-barrier");
    let server = FakeServer::start(vec![
        Script::Immediate(sse::tool_call("call_1", sse::SHELL_EXEC_WIRE_NAME, r#"{"cmd": "echo ok"}"#)),
        Script::Immediate(sse::text_reply("done after tool")),
    ]);
    let providers = scratch.write_providers_toml(&server.base_url());
    let session = scratch.path("s.jsonl");

    let mut cli = CliProcess::spawn(&providers, Some(&session));
    assert!(cli.wait_for("输入一句话开始对话", T), "启动横幅超时：{}", cli.combined_output());

    cli.send_line("please run shell");
    assert!(cli.wait_for("[本轮完成]", T), "带工具调用的一轮没完成：{}", cli.combined_output());
    let after_turn = cli.stdout_snapshot();
    assert!(after_turn.contains("srv:shell/exec"), "该看到工具执行日志：{after_turn}");
    assert_eq!(
        after_turn.matches("srv:shell/exec 完成").count(),
        1,
        "shell 命令这一步该恰好被真正执行一次：{after_turn}"
    );

    // 越过屏障前：/undo 该停在门口，输出点名工具与 call_id（“撤销受阻”就是
    // undo_blocked 在这份 CLI 里的措辞）。
    cli.send_line("/undo");
    assert!(cli.wait_for("[撤销受阻]", T), "该被屏障挡住：{}", cli.combined_output());
    let blocked = cli.stdout_snapshot();
    assert!(blocked.contains("srv:shell/exec"), "撤销受阻消息该点名工具：{blocked}");
    assert!(blocked.contains("call_1"), "撤销受阻消息该带 call_id：{blocked}");
    assert!(blocked.contains("/undo!"), "撤销受阻消息该给出明确的越过指引：{blocked}");

    // 该轮没有被回退：/redo 能找回刚才因为撞屏障而被局部撤销的那部分，
    // 不是“无事可做”，证明净状态跟撞屏障之前一致。
    cli.send_line("/redo");
    assert!(cli.wait_for("[已重做]", T), "该能重做回撞屏障之前的状态：{}", cli.combined_output());
    assert!(!cli.stdout_snapshot().contains("没有可重做的了"), "不该是无事可做");

    // `/undo!`：显式越过，整轮真正被撤销，且明确告知越过的是哪个不可逆操作。
    cli.send_line("/undo!");
    assert!(cli.wait_for("[已越过]", T), "该报告越过了屏障：{}", cli.combined_output());
    let forced = cli.stdout_snapshot();
    assert!(forced.contains("[已撤销]"), "/undo! 之后整轮该被撤销：{forced}");
    assert!(forced.contains("srv:shell/exec"), "越过提示该点名工具：{forced}");
    assert!(forced.contains("call_1"), "越过提示该带 call_id：{forced}");

    cli.send_line("/quit");
    assert!(cli.wait_exit(T).is_some(), "该干净退出：{}", cli.combined_output());

    // 屏障/undo/redo/undo! 全部是本地原子回滚操作，不产生新的网络请求——
    // 全程只有最初那一轮的两次 provider 调用（工具调用 1 次 + 结果后的最终
    // 回复 1 次）。
    assert_eq!(server.bodies().len(), 2, "undo 系命令不该触发新的网络请求");
    // 全程 shell 命令只被真正执行了一次（在最终的合并输出里同样只出现一次
    // “完成”标记），undo!/redo 不会让它被悄悄重跑。
    let full = cli.combined_output();
    assert_eq!(full.matches("srv:shell/exec 完成").count(), 1, "整个会话期间 shell 只该被真正执行一次：{full}");
}
