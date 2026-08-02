//! 验收清单第二条：同一个 session 的两个订阅者收到同一序列事件。两边都在
//! 发命令之前订阅（`broadcast` 没有历史重放，见 `SessionHandle::subscribe`
//! 文档），之后各自收到的事件序列必须逐条相等。

mod support;

use std::time::Duration;

use support::server::{FakeServer, Script};
use support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn two_subscribers_of_the_same_session_get_identical_event_sequences() {
    let server = FakeServer::start(vec![Script::Immediate(text_reply("hello"))]);
    let registry = agent_server::SessionRegistry::new();
    let handle = registry.open(support::open_spec("shared", server.endpoint(), None)).unwrap();

    let mut sub1 = handle.subscribe();
    let mut sub2 = handle.subscribe();

    handle.send(agent_server::Command::Input("hi".to_string())).unwrap();

    let events1 = support::collect_until_terminal(&mut sub1, Duration::from_secs(5)).await;
    let events2 = support::collect_until_terminal(&mut sub2, Duration::from_secs(5)).await;

    assert!(!events1.is_empty());
    assert_eq!(events1, events2, "两个订阅者该看到完全相同的事件序列");
}
