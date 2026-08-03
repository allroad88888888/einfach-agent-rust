//! 验收清单第五条：undo/redo/cancel 端点各自生效（复用 030 的命令语义）。三个
//! 端点结构上高度相似（都是 fire-and-forget POST，结果走 SSE），放一个文件里。

mod support;

use std::time::Duration;

use agent_core::{Failure, Notice, TurnStatus};
use agent_server::{Frame, SessionEvent, UndoOutcome};

use support::http_client;
use support::server::{FakeServer, Script};
use support::wire::text_reply;

async fn create_session_with_sse(addr: std::net::SocketAddr) -> (String, http_client::SseReader) {
    let create = http_client::request(addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");
    let (status, _, sse) = http_client::connect_sse(addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200);
    (id, sse)
}

/// 034：SSE 帧 data 是 `Frame` 信封，不再是裸的 `SessionEvent`。
///
/// 048 起 undo/redo 之后、以及每轮进行中都会穿插 `SessionEvent::AgentTree` 快照帧
/// （活树面板/`GET .../agents` 的数据源）。这个文件测的是 undo/redo/cancel 的**命令
/// 语义**，树帧对它们是噪声——跳过它，专门断言下一条真正关心的事件。树帧本身由
/// `http_agent_tree_get_matches_sse` / `tree_snapshot_emits_on_change` 专门测。
fn next_typed(sse: &mut http_client::SseReader, budget: Duration) -> Frame {
    loop {
        let frame = sse.next_event(budget).expect("该收到一帧");
        let parsed: Frame = serde_json::from_str(&frame.data).unwrap_or_else(|e| panic!("{e}: {}", frame.data));
        if matches!(parsed.event, SessionEvent::AgentTree(_)) {
            continue;
        }
        return parsed;
    }
}

async fn run_one_turn_to_completion(addr: std::net::SocketAddr, id: &str, sse: &mut http_client::SseReader) {
    let input = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);
    loop {
        let frame = next_typed(sse, Duration::from_secs(5));
        if matches!(&frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()) {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_endpoint_reverts_the_last_turn() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("done"))]);
    let server = support::http_server::start(upstream.endpoint()).await;
    let (id, mut sse) = create_session_with_sse(server.addr).await;
    run_one_turn_to_completion(server.addr, &id, &mut sse).await;

    let undo = http_client::request(server.addr, "POST", &format!("/sessions/{id}/undo"), Some("{}"));
    assert_eq!(undo.status, 202, "{}", undo.body);

    let frame = next_typed(&mut sse, Duration::from_secs(3));
    assert!(matches!(frame.event, SessionEvent::Undo(UndoOutcome::Applied { .. })), "{frame:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn redo_endpoint_reapplies_the_undone_turn() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("done"))]);
    let server = support::http_server::start(upstream.endpoint()).await;
    let (id, mut sse) = create_session_with_sse(server.addr).await;
    run_one_turn_to_completion(server.addr, &id, &mut sse).await;

    let undo = http_client::request(server.addr, "POST", &format!("/sessions/{id}/undo"), Some("{}"));
    assert_eq!(undo.status, 202);
    let frame = next_typed(&mut sse, Duration::from_secs(3));
    assert!(matches!(frame.event, SessionEvent::Undo(UndoOutcome::Applied { .. })), "{frame:?}");

    let redo = http_client::request(server.addr, "POST", &format!("/sessions/{id}/redo"), None);
    assert_eq!(redo.status, 202, "{}", redo.body);
    let frame = next_typed(&mut sse, Duration::from_secs(3));
    assert!(matches!(frame.event, SessionEvent::Redo(UndoOutcome::Applied { .. })), "{frame:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_endpoint_stops_the_flying_turn() {
    let upstream = FakeServer::start(vec![Script::HangAfterHeaders]);
    let server = support::http_server::start(upstream.endpoint()).await;
    let (id, mut sse) = create_session_with_sse(server.addr).await;

    let input = http_client::request(server.addr, "POST", &format!("/sessions/{id}/input"), Some("{\"text\":\"hi\"}"));
    assert_eq!(input.status, 202);

    let cancel = http_client::request(server.addr, "POST", &format!("/sessions/{id}/cancel"), Some("{}"));
    assert_eq!(cancel.status, 202, "{}", cancel.body);

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut cancelled = false;
    while std::time::Instant::now() < deadline {
        let frame = next_typed(&mut sse, Duration::from_secs(3));
        if matches!(frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status: TurnStatus::Failed(Failure::Cancelled) })) {
            cancelled = true;
            break;
        }
    }
    assert!(cancelled, "POST /cancel 该让在飞的轮次几百毫秒内 Failed(Cancelled)，不用等 provider 超时");
}
