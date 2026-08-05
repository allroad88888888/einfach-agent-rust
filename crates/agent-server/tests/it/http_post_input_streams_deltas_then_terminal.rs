//! 验收清单第一条：假浏览器（原生 `TcpStream` 客户端）先连 SSE，`POST input`
//! 之后在 SSE 上收到增量帧序列、最后是轮终态帧。顺带钉住 `POST /sessions` →
//! `GET /sessions/:id` 这条创建-查询链路（issue 031「做什么」小节点名的两个
//! 会话生命周期端点）。

use crate::support;
use std::time::Duration;

use agent_core::{Notice, TurnStatus};
use agent_server::{Frame, SessionEvent};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn posting_input_then_reading_sse_yields_deltas_then_a_terminal_frame() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("hello from the model"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");
    assert!(!id.is_empty(), "该拿到一个非空 session id：{}", create.body);

    let status = http_client::request(server.addr, "GET", &format!("/sessions/{id}"), None);
    assert_eq!(status.status, 200);
    assert!(status.body.contains("\"alive\""), "{}", status.body);

    let (sse_status, headers, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(sse_status, 200);
    assert!(
        headers
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("cache-control")
                && v.eq_ignore_ascii_case("no-cache")),
        "{headers:?}"
    );

    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some("{\"text\":\"hi\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    let mut text = String::new();
    let mut terminal: Option<TurnStatus> = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline && terminal.is_none() {
        let Some(frame) = sse.next_event(Duration::from_secs(5)) else {
            break;
        };
        assert!(frame.id.is_some(), "服务端每一帧都该带 id：{frame:?}");
        // 034：SSE 帧 data 是 `Frame` 信封，不再是裸的 `SessionEvent`。
        let envelope: Frame = serde_json::from_str(&frame.data)
            .unwrap_or_else(|e| panic!("反序列化失败：{e}：{}", frame.data));
        match envelope.event {
            SessionEvent::TextDelta(delta) => text.push_str(&delta),
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal() => {
                terminal = Some(status)
            }
            _ => {}
        }
    }

    assert_eq!(text, "hello from the model");
    assert!(
        matches!(terminal, Some(TurnStatus::Done { .. })),
        "该以正常结束的轮终态收尾：{terminal:?}"
    );
}
