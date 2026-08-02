//! 验收清单第三条后半：`Last-Event-ID` 超出环形缓冲还留着的范围 → 一帧显式
//! `gap` 事件，不是假装什么都没发生。`support::http_server::start` 把环形缓冲
//! 容量调到 5 帧，好在测试里真的把它挤爆而不用发几百条消息。

mod support;

use std::time::Duration;

use agent_core::AgentId;
use agent_server::{Frame, SessionEvent};

use support::http_client;
use support::server::{FakeServer, Script};
use support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn a_last_event_id_older_than_the_ring_buffer_yields_an_explicit_gap_frame() {
    // 一条脚本就够——`FakeServer` 耗尽脚本列表之后重复最后一条（`support::server`
    // 模块文档），后面 7 轮都吃这一条。
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("hi"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    // 不连 SSE，直接跑好几轮——环形缓冲容量是 5（`http_server::start`），跑够
    // 多轮之后，id 1 早就被挤出去了。
    for _ in 0..8 {
        let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
        assert_eq!(input.status, 202);
        tokio::time::sleep(Duration::from_millis(30)).await; // 给这一轮时间跑完再发下一句
    }

    let (status, _, mut sse) = http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), Some(1));
    assert_eq!(status, 200);

    let frame = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    // 034：SSE 帧 data 是 `Frame` 信封，不再是裸的 `SessionEvent`。
    let envelope: Frame = serde_json::from_str(&frame.data).unwrap_or_else(|e| panic!("{e}: {}", frame.data));
    assert!(matches!(envelope.event, SessionEvent::Gap { .. }), "缓冲区早就挤掉 id 1 了，第一帧该是 Gap：{envelope:?}");
    assert_eq!(envelope.agent, AgentId::root(), "gap 帧是重连补发算出来的传输层事实，该标 root");
}
