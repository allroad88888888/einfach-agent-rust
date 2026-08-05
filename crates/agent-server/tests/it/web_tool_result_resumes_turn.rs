//! Web 工具的闭环：SSE 派发 `browser_action`，宿主通过 `/tool_result` 回传，
//! 同一轮才继续第二次 provider 调用。此测试也证明 HTTP 不能直接伪造 epoch。

mod support;

use std::time::Duration;

use agent_server::{Frame, SessionEvent, ToolTableSpec};

use support::http_client;
use support::server::{FakeServer, Script};

fn browser_action_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_browser_1","type":"function","function":{"name":"browser_action","arguments":"{\"action\":\"render_card\",\"payload\":{\"title\":\"Hello\"}}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> Frame {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data))
}

#[tokio::test(flavor = "multi_thread")]
async fn browser_action_result_is_matched_then_resumes_the_waiting_turn() {
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

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");
    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200);

    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some("{\"text\":\"展示卡片\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    let (agent, call_id) = loop {
        let frame = next_frame(&mut sse);
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event {
            assert_eq!(&*request.tool, "browser_action");
            break (frame.agent, call_id);
        }
    };
    let body = serde_json::json!({
        "agent": agent,
        "tool_call_id": call_id,
        "result": { "content": "{\"cardId\":\"card-1\"}" },
    })
    .to_string();
    let result = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_result"),
        Some(&body),
    );
    assert_eq!(result.status, 202, "{}", result.body);

    let mut saw_result = false;
    loop {
        let frame = next_frame(&mut sse);
        saw_result |= matches!(&frame.event, SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == "browser_action");
        if matches!(&frame.event, SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal())
        {
            break;
        }
    }
    assert!(saw_result, "必须先把 Web 回传记成工具结果再结束本轮");
    assert_eq!(
        upstream.request_count(),
        2,
        "远端结果应触发同一轮的第二次 provider 调用"
    );
}
