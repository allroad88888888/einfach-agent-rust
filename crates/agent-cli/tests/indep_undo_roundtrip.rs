//! 独立测试 agent（issue 027）验收点 1：
//! “CLI 十轮对话后 /undo：上一轮消失、派生值一致、下一轮 prompt 不含被退内容”
//! ——这里用两轮（脚本固定回复）走通同一条黑盒证法：`/undo` 之后输出要说明
//! 退了哪一轮、几条，再问一轮，假服务器收到的第 3 个请求体里不能含被退轮的
//! 用户输入和助手回复（服务器把收到的 body 存下来断言，这就是“下一轮
//! prompt 不含被退内容”的黑盒证法）。

mod indep_support;

use indep_support::{CliProcess, FakeServer, Scratch, Script, sse};
use std::time::Duration;

const T: Duration = Duration::from_secs(10);

#[test]
fn undo_removes_the_previous_turn_and_the_next_request_excludes_it() {
    let scratch = Scratch::new("undo-roundtrip");
    let server = FakeServer::start(vec![
        Script::Immediate(sse::text_reply("turn one reply")),
        Script::Immediate(sse::text_reply("turn two reply")),
        Script::Immediate(sse::text_reply("turn three reply")),
    ]);
    let providers = scratch.write_providers_toml(&server.base_url());
    let session = scratch.path("s.jsonl");

    let mut cli = CliProcess::spawn(&providers, Some(&session));
    assert!(cli.wait_for("输入一句话开始对话", T), "启动横幅超时：{}", cli.combined_output());

    cli.send_line("first message");
    assert!(cli.wait_for("[本轮完成]", T), "第一轮没完成：{}", cli.combined_output());

    cli.send_line("second message");
    assert!(cli.wait_for("turn two reply", T), "第二轮没收到回复：{}", cli.combined_output());
    assert!(cli.wait_for("[本轮完成]", T), "第二轮没完成：{}", cli.combined_output());

    cli.send_line("/undo");
    assert!(cli.wait_for("[已撤销]", T), "没看到撤销提示：{}", cli.combined_output());
    let after_undo = cli.stdout_snapshot();
    assert!(after_undo.contains("第 2 轮"), "撤销输出该说明退了第几轮：{after_undo}");
    assert!(after_undo.contains('条'), "撤销输出该说明退了几条：{after_undo}");

    cli.send_line("third message");
    assert!(cli.wait_for("[本轮完成]", T), "第三轮没完成：{}", cli.combined_output());

    cli.send_line("/quit");
    assert!(cli.wait_exit(T).is_some(), "该干净退出：{}", cli.combined_output());

    let bodies = server.bodies();
    assert_eq!(bodies.len(), 3, "应该恰好发生 3 次网络请求：{bodies:?}");
    let third = &bodies[2];
    assert!(third.contains("first message"), "第 3 个请求体该保留第一轮：{third}");
    assert!(third.contains("turn one reply"), "第 3 个请求体该保留第一轮的回复：{third}");
    assert!(!third.contains("second message"), "第 3 个请求体不该含被撤销轮的用户输入：{third}");
    assert!(!third.contains("turn two reply"), "第 3 个请求体不该含被撤销轮的助手回复：{third}");
    assert!(third.contains("third message"), "第 3 个请求体该含新一轮的输入：{third}");
}
