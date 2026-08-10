//! 验收清单第一条：两个 session 并行各自对话（假 SSE），事件互不串台，store
//! 线程互不共享。两个假服务器各自只认自己的那家会话，靠回复文本互不相同来
//! 证明「session A 的订阅者只看得到 A 的文本」。

use crate::support;
use std::time::Duration;

use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn two_sessions_run_concurrently_without_crossing_events() {
    let server_a = FakeServer::start(vec![Script::Immediate(text_reply("from-A"))]);
    let server_b = FakeServer::start(vec![Script::Immediate(text_reply("from-B"))]);

    let registry = agent_server::SessionRegistry::new();
    let handle_a = registry
        .open(support::open_spec("session-a", server_a.endpoint(), None))
        .unwrap();
    let handle_b = registry
        .open(support::open_spec("session-b", server_b.endpoint(), None))
        .unwrap();

    let mut sub_a = handle_a.subscribe();
    let mut sub_b = handle_b.subscribe();

    handle_a
        .send(agent_server::Command::Input {
            text: "hi from a".to_string(),
        })
        .unwrap();
    handle_b
        .send(agent_server::Command::Input {
            text: "hi from b".to_string(),
        })
        .unwrap();

    let events_a = support::collect_until_terminal(&mut sub_a, Duration::from_secs(5)).await;
    let events_b = support::collect_until_terminal(&mut sub_b, Duration::from_secs(5)).await;

    let text_a = support::text_of(&events_a);
    let text_b = support::text_of(&events_b);

    assert_eq!(text_a, "from-A");
    assert_eq!(text_b, "from-B");
    assert!(
        !text_a.contains("from-B"),
        "session A 的订阅者不该看到 B 的文本"
    );
    assert!(
        !text_b.contains("from-A"),
        "session B 的订阅者不该看到 A 的文本"
    );

    // 各自只打过一次请求——两个 store 线程互不共享，互不重复对方的工作量。
    assert_eq!(server_a.request_count(), 1);
    assert_eq!(server_b.request_count(), 1);
}
