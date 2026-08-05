//! 独立测试 agent（issue 027）验收点 4：
//! “流中途（假服务器慢发）给子进程 SIGINT → 输出含取消/擦除提示 → 下一轮
//! 请求体不含被取消轮的用户输入”。
//!
//! `Cargo.toml` 顶部注释确认 Ctrl-C 走的是真信号捕捉（`ctrlc` crate），所以
//! 测试用系统 `kill -INT <pid>` 直接给子进程发真信号，效果跟终端里按
//! Ctrl-C 完全一样。

use crate::indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[test]
fn sigint_mid_stream_erases_the_cancelled_turn_and_the_next_request_excludes_it() {
    let scratch = Scratch::new("cancel-erase");
    let server = FakeServer::start(vec![
        Script::StallThenFinish {
            first: "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"},\"finish_reason\":null}]}\n\n".to_string(),
            // 卡住足够久：测试只需要它在断言全部跑完之前别把连接关掉就行，
            // 具体多久不重要，因为客户端会在取消标志置位后主动断开。
            stall: Duration::from_secs(20),
            rest: "data: {\"choices\":[{\"delta\":{\"content\":\" more\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string(),
        },
        Script::Immediate(sse::text_reply("after cancel reply")),
    ]);
    let providers = scratch.write_providers_toml(&server.base_url());
    let session = scratch.path("s.jsonl");

    let mut cli = CliProcess::spawn(&providers, Some(&session));
    assert!(
        cli.wait_for("输入一句话开始对话", T),
        "启动横幅超时：{}",
        cli.combined_output()
    );

    cli.send_line("cancel me please");
    assert!(
        cli.wait_for("partial", T),
        "该先收到部分流式内容：{}",
        cli.combined_output()
    );
    // 给 fire-and-forget 落盘一点余量，确保“工具调用已经开始”这件事本身
    // （这里是 provider 流已经落了部分内容）有机会先写到内存状态里。
    std::thread::sleep(Duration::from_millis(150));
    cli.send_signal("-INT");

    assert!(
        cli.wait_for("[已撤销]", T),
        "该看到取消之后的擦除提示：{}",
        cli.combined_output()
    );
    let after_cancel = cli.stdout_snapshot();
    assert!(
        after_cancel.contains("Cancelled") || after_cancel.contains("取消"),
        "该有取消语义：{after_cancel}"
    );
    assert!(
        after_cancel.contains("擦除"),
        "该有擦除语义：{after_cancel}"
    );

    cli.send_line("after cancel message");
    assert!(
        cli.wait_for("[本轮完成]", T),
        "取消后该能正常继续对话：{}",
        cli.combined_output()
    );

    cli.send_line("/quit");
    assert!(
        cli.wait_exit(T).is_some(),
        "该干净退出：{}",
        cli.combined_output()
    );

    let bodies = server.bodies();
    assert!(
        bodies.len() >= 2,
        "至少该发生“被取消的一轮”+“取消后新一轮”两次请求：{bodies:?}"
    );
    let last = bodies.last().unwrap();
    assert!(
        !last.contains("cancel me please"),
        "被取消轮的用户输入不该出现在下一轮请求体里：{last}"
    );
    assert!(
        last.contains("after cancel message"),
        "下一轮请求体该含新输入：{last}"
    );
}
