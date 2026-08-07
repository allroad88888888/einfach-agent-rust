//! 验收清单第四条：`Input` 在轮进行中到达 → 排队，当前轮结束后按序处理
//! （不丢不并发）。背靠背发两条 `Input`，不等第一条跑完——`mpsc` 是 FIFO，
//! actor 是单线程循环，第二条物理上不可能提前于第一条被处理；这个测试证明
//! 的是「两条都真的各自跑完了一整轮，而不是后一条把前一条挤丢/合并」。

use crate::support;
use std::time::Duration;

use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn two_inputs_sent_back_to_back_both_run_and_in_submission_order() {
    let server = FakeServer::start(vec![
        Script::Immediate(text_reply("first-reply")),
        Script::Immediate(text_reply("second-reply")),
    ]);
    let registry = agent_server::SessionRegistry::new();
    let handle = registry
        .open(support::open_spec("queue-me", server.endpoint(), None))
        .unwrap();

    let mut sub = handle.subscribe();
    handle
        .send(agent_server::Command::Input {
            text: "first".to_string(),
        })
        .unwrap();
    handle
        .send(agent_server::Command::Input {
            text: "second".to_string(),
        })
        .unwrap(); // 不等第一条的结果

    let first_turn = support::collect_until_terminal(&mut sub, Duration::from_secs(5)).await;
    let second_turn = support::collect_until_terminal(&mut sub, Duration::from_secs(5)).await;

    assert_eq!(support::text_of(&first_turn), "first-reply");
    assert_eq!(support::text_of(&second_turn), "second-reply");

    // 两次真实网络请求，按提交顺序到达——第一条请求体里有 "first"，
    // 第二条里有 "second"，不是反过来、也不是被合并成一条。
    let bodies = server.bodies();
    assert_eq!(bodies.len(), 2, "该有两次独立的 provider 调用，一轮一次");
    assert!(
        bodies[0].contains("first"),
        "第一次请求体该含第一条输入：{}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("second"),
        "第二次请求体该含第二条输入：{}",
        bodies[1]
    );
}
