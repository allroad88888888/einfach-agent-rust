//! 验收清单第二条：两个 SSE 客户端同帧序；断一个不影响另一个。

mod support;

use std::time::Duration;

use agent_server::{Frame, SessionEvent};

use support::http_client;
use support::server::{FakeServer, Script};
use support::wire::text_reply;

/// 034：SSE 帧 data 是 `Frame` 信封，不再是裸的 `SessionEvent`——返回整个信封
/// （不只 `event`）：「两个订阅者该看到完全相同的事件序列」这条断言这样也顺带
/// 覆盖了 agent 归属一致，不只是事件内容一致。
async fn collect_all(sse: &mut http_client::SseReader, budget: Duration) -> Vec<Frame> {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + budget;
    while let Some(frame) = sse.next_event(Duration::from_secs(5)) {
        let envelope: Frame = serde_json::from_str(&frame.data).expect("每一帧都该是合法的 Frame JSON");
        let terminal = matches!(&envelope.event, SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal());
        out.push(envelope);
        if terminal || std::time::Instant::now() >= deadline {
            break;
        }
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn two_sse_clients_of_the_same_session_see_the_same_sequence() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("same for both"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let (status1, _, mut sse1) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    let (status2, _, mut sse2) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status1, 200);
    assert_eq!(status2, 200);

    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    let events1 = collect_all(&mut sse1, Duration::from_secs(5)).await;
    let events2 = collect_all(&mut sse2, Duration::from_secs(5)).await;

    assert!(!events1.is_empty());
    assert_eq!(events1, events2, "两个订阅者该看到完全相同的事件序列");
}

#[tokio::test(flavor = "multi_thread")]
async fn one_client_disconnecting_does_not_affect_the_other() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("still here"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let (_, _, sse1) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    let (_, _, mut sse2) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);

    drop(sse1); // 断开第一个（还剩 sse2 一个订阅者，宽限计时不该起）

    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    let events2 = collect_all(&mut sse2, Duration::from_secs(5)).await;
    assert!(!events2.is_empty(), "剩下的订阅者该正常收到这一轮的事件");
    let text: String = events2
        .iter()
        .filter_map(|f| match &f.event {
            SessionEvent::TextDelta(d) => Some(d.as_ref()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "still here");
}
