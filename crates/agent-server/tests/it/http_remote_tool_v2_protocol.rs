//! 092-B：远端工具 v2 的 HTTP 闭环必须取得 actor 的同步确认，不是仅仅排队。

use std::time::Duration;

use agent_core::Notice;
use agent_server::{Frame, SessionEvent, ToolTableSpec};
use serde_json::{Value, json};

use crate::support;
use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CLAIM_ID: &str = "executor-a";
const SUBMISSION_ID: &str = "submission-a";

fn browser_action_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_browser_1","type":"function","function":{"name":"browser_action","arguments":"{\"action\":\"render_card\",\"token\":\"status-raw-request-canary\"}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}]}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> Frame {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data))
}

fn error_code(body: &str) -> String {
    serde_json::from_str::<Value>(body).expect("错误必须是 JSON")["error"]["code"]
        .as_str()
        .expect("错误必须带稳定 code")
        .to_owned()
}

fn endpoint(id: &str, agent: &str, call_id: &str) -> String {
    format!("/sessions/{id}/tool_status?agent={agent}&tool_call_id={call_id}")
}

#[tokio::test(flavor = "multi_thread")]
async fn v2_claim_submit_replay_conflict_and_status_are_actor_confirmed() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(browser_action_reply()),
        Script::Immediate(support::wire::text_reply("已渲染。")),
    ]);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = ToolTableSpec::Standard;
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |config| config,
    )
    .await;

    let created = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(created.status, 201, "{}", created.body);
    let id = support::extract_json_string_field(&created.body, "id");
    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200);
    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some(r#"{"text":"展示卡片"}"#),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    let (agent, call_id) = loop {
        let frame = next_frame(&mut sse);
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event
            && &*request.tool == "browser_action"
        {
            break (frame.agent.0.to_string(), call_id.0.to_string());
        }
    };
    let claim = json!({ "agent": agent, "tool_call_id": call_id, "claim_id": CLAIM_ID });
    let first_claim = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_claim"),
        Some(&claim.to_string()),
    );
    assert_eq!(first_claim.status, 200, "{}", first_claim.body);
    let first_claim: Value = serde_json::from_str(&first_claim.body).unwrap();
    assert_eq!(first_claim["disposition"], "claimed");
    assert_eq!(first_claim["request"]["tool"], "browser_action");

    let other_claim = json!({ "agent": agent, "tool_call_id": call_id, "claim_id": "executor-b" });
    let other_claim = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_claim"),
        Some(&other_claim.to_string()),
    );
    assert_eq!(other_claim.status, 200, "{}", other_claim.body);
    let other_claim: Value = serde_json::from_str(&other_claim.body).unwrap();
    assert_eq!(other_claim["disposition"], "ignored");

    let active = http_client::request_with_headers(
        server.addr,
        "GET",
        &endpoint(&id, &agent, &call_id),
        &[("x-tool-claim-id", CLAIM_ID)],
        None,
    );
    assert_eq!(active.status, 200, "{}", active.body);
    let active: Value = serde_json::from_str(&active.body).unwrap();
    assert_eq!(active["state"], "claimed");
    assert_eq!(active["claimed_by_me"], true);
    assert_eq!(active["revision"], first_claim["revision"]);
    assert!(active["request"].is_null());
    assert!(
        !active.to_string().contains("status-raw-request-canary"),
        "status projection must not expose raw tool input"
    );

    let result = json!({
        "agent": agent,
        "tool_call_id": call_id,
        "claim_id": CLAIM_ID,
        "submission_id": SUBMISSION_ID,
        "outcome": { "status": "succeeded", "content": "{\"cardId\":\"card-1\"}" }
    });
    let committed = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_result"),
        Some(&result.to_string()),
    );
    assert_eq!(committed.status, 200, "{}", committed.body);
    let committed: Value = serde_json::from_str(&committed.body).unwrap();
    assert_eq!(committed["disposition"], "committed");
    assert_eq!(committed["terminal_status"], "succeeded");

    let duplicate = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_result"),
        Some(&result.to_string()),
    );
    assert_eq!(duplicate.status, 200, "{}", duplicate.body);
    let duplicate: Value = serde_json::from_str(&duplicate.body).unwrap();
    assert_eq!(duplicate["disposition"], "duplicate");
    assert_eq!(duplicate["revision"], committed["revision"]);

    let conflict = json!({
        "agent": agent,
        "tool_call_id": call_id,
        "claim_id": CLAIM_ID,
        "submission_id": SUBMISSION_ID,
        "outcome": { "status": "succeeded", "content": "different" }
    });
    let conflict = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_result"),
        Some(&conflict.to_string()),
    );
    assert_eq!(conflict.status, 409, "{}", conflict.body);
    assert_eq!(error_code(&conflict.body), "result_conflict");

    let terminal = http_client::request_with_headers(
        server.addr,
        "GET",
        &endpoint(&id, &agent, &call_id),
        &[("x-tool-claim-id", CLAIM_ID)],
        None,
    );
    assert_eq!(terminal.status, 200, "{}", terminal.body);
    let terminal: Value = serde_json::from_str(&terminal.body).unwrap();
    assert_eq!(terminal["state"], "succeeded");
    assert_eq!(terminal["submission_id"], SUBMISSION_ID);
    assert_eq!(terminal["terminal_origin"], "host");
    assert_eq!(terminal["revision"], committed["revision"]);

    let mut saw_committed_event = false;
    loop {
        let frame = next_frame(&mut sse);
        saw_committed_event |= matches!(
            &frame.event,
            SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == "browser_action"
        );
        if matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        ) {
            break;
        }
    }
    assert!(
        saw_committed_event,
        "committed 响应对应的工具结果必须进入 core 事件流"
    );
    assert_eq!(
        upstream.request_count(),
        2,
        "提交后必须恢复同一轮 provider 调用"
    );
}
