//! 验收清单第三条前半：重连带 `Last-Event-ID` → 精确补发缺的帧，帧内容逐字节
//! 同首播。

mod support;

use std::time::Duration;

use agent_core::Notice;
use agent_server::{Frame, SessionEvent};

use support::http_client;
use support::server::{FakeServer, Script};
use support::wire::text_reply;

/// 反序列化成真的 `Frame`（而不是子串匹配）才知道是不是终态——嵌套的
/// `agent_core::Notice` 用的是它自己的 serde 默认命名（`TurnStatusChanged`，
/// 不是本 crate 给 `SessionEvent` 定的 snake_case tag，两者是两层协议），子串
/// 猜测很容易猜错大小写/命名风格。034：SSE 帧 data 是 `Frame` 信封
/// （`{"agent":...,"event":{...}}`），解析目标从 `SessionEvent` 换成 `Frame`。
fn is_terminal(data: &str) -> bool {
    match serde_json::from_str::<Frame>(data) {
        Ok(Frame { event: SessionEvent::Notice(Notice::TurnStatusChanged { status }), .. }) => status.is_terminal(),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnecting_with_last_event_id_replays_exactly_the_missed_frames_byte_for_byte() {
    // 长一点的回复，好凑出好几帧 text_delta——只有一帧就没有「中间断开、还有
    // 剩下的帧要补」这回事。
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("one two three four five"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let (_, _, mut first) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    // 先首播全部：这是「首播」的基准序列，重连之后要跟这里剩下的部分逐字节对上。
    let mut first_seen = Vec::new();
    while let Some(frame) = first.next_event(Duration::from_secs(5)) {
        let terminal = is_terminal(&frame.data);
        first_seen.push(frame);
        if terminal {
            break;
        }
    }
    assert!(first_seen.len() >= 2, "至少要有几帧才能测『断在中间、补剩下的』：{}", first_seen.len());

    // 断开首播连接（不再读它），假装从中间某一帧开始重连——只保留前一半，
    // 用它的最后一个 id 当 Last-Event-ID。
    drop(first);
    let split = first_seen.len() / 2;
    let last_seen_id = first_seen[split - 1].id.expect("每一帧都带 id");
    let expected_replay = &first_seen[split..];

    let (status, _, mut reconnected) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), Some(last_seen_id));
    assert_eq!(status, 200);

    let mut replayed = Vec::new();
    for _ in 0..expected_replay.len() {
        let Some(frame) = reconnected.next_event(Duration::from_secs(5)) else { break };
        replayed.push(frame);
    }

    assert_eq!(replayed, expected_replay, "重连补发的帧该跟首播剩下的部分逐字节相同（id 和 data 都要对上）");
}
