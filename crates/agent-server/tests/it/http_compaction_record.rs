//! 109 独立测试点：`GET /sessions/:id/compaction_record` 的基础接线——
//! actor 邮箱查询往返（新的 `ActorMessage::ReadCompactionRecord`）真的把
//! `Session::messages_of`/`summary_library` 现查出来的值带回 HTTP 响应体，
//! 不是一份写死的空壳；未知会话 404，跟 `agents`/`pending_tools` 两个既有
//! GET 端点同一条判据（`AppState::session_handle`）。
//!
//! 不在这条里驱动真实压缩（需要一整套摘要子 agent + 假上游脚本，`compact_
//! ladder`/`compact_writeback` 已经在 `agent-runtime` 单元测得很细）——这里
//! 钉住的是「HTTP 查询确实读到了这个会话此刻的完整记录」这一层管线。

use crate::support;

use agent_core::Notice;
use agent_server::{Frame, SessionEvent};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_session_id_is_404() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let res = http_client::request(
        server.addr,
        "GET",
        "/sessions/never-existed/compaction_record",
        None,
    );
    assert_eq!(res.status, 404, "{}", res.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_session_has_an_empty_record() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");

    let res = http_client::request(
        server.addr,
        "GET",
        &format!("/sessions/{id}/compaction_record"),
        None,
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body: serde_json::Value = serde_json::from_str(&res.body).unwrap();
    assert_eq!(body["messages"], serde_json::json!([]), "{}", res.body);
    assert_eq!(body["summaries"], serde_json::json!([]), "{}", res.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_record_reflects_this_sessions_live_messages() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("晴天"))]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let (sse_status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(sse_status, 200);

    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some("{\"text\":\"今天天气怎么样\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    // 排干净这一轮的 SSE 帧,等到轮终态——这样下面的 GET 落在「一轮已经真的
    // 写进 Slot::Messages」之后,不是竞态地问一个可能还没到的状态。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut terminal = false;
    while std::time::Instant::now() < deadline && !terminal {
        let Some(raw) = sse.next_event(std::time::Duration::from_secs(5)) else {
            break;
        };
        let frame: Frame =
            serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data));
        terminal = matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        );
    }
    assert!(terminal, "该在 5 秒内看到轮终态帧");

    let res = http_client::request(
        server.addr,
        "GET",
        &format!("/sessions/{id}/compaction_record"),
        None,
    );
    assert_eq!(res.status, 200, "{}", res.body);
    let body: serde_json::Value = serde_json::from_str(&res.body).unwrap();

    let messages = body["messages"].as_array().expect("messages 该是数组");
    assert_eq!(messages.len(), 2, "一句用户输入 + 一句终答：{}", res.body);
    assert_eq!(messages[0]["role"], "User");
    assert_eq!(messages[1]["role"], "Assistant");
    let user_text = &messages[0]["blocks"][0]["Text"];
    assert_eq!(user_text, "今天天气怎么样", "{}", res.body);
    let assistant_text = &messages[1]["blocks"][0]["Text"];
    assert_eq!(assistant_text, "晴天", "{}", res.body);

    // 没有发生过任何压缩,摘要库该仍是空的——这一条只是确认两个字段互不干扰。
    assert_eq!(body["summaries"], serde_json::json!([]), "{}", res.body);
}
