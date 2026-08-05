//! Host skill 的延迟工具只在当前 agent 激活期间可调度，并复用既有远端工具闭环。

use crate::support;
use std::time::Duration;

use agent_core::{Location, Notice, Reversibility};
use agent_server::{Frame, SessionEvent};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

const CHAT_ID: &str = "host-skill-dispatch";
const SKILL: &str = "source-diagnostics";
const SOURCE_TOOL: &str = "web:source/read";
const ACTIVATE: &str = "srv:skill/activate";
const DEACTIVATE: &str = "srv:skill/deactivate";

fn tool_reply(call_id: &str, wire_name: &str, arguments: Value) -> String {
    let call = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": { "name": wire_name, "arguments": arguments.to_string() }
                }]
            },
            "finish_reason": Value::Null
        }]
    });
    let finish = json!({
        "choices": [{ "index": 0, "delta": { "content": "" }, "finish_reason": "tool_calls" }]
    });
    format!("data: {call}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> (u64, Frame) {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    let id = raw.id.expect("服务端每帧都应有游标");
    let frame =
        serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data));
    (id, frame)
}

fn drain_until_terminal(sse: &mut http_client::SseReader) -> Vec<Frame> {
    let mut frames = Vec::new();
    loop {
        let (_, frame) = next_frame(sse);
        let terminal = matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        );
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
}

fn input(server: &support::http_server::TestServer, text: &str) {
    let body = json!({ "text": text }).to_string();
    let response = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(&body),
    );
    assert_eq!(response.status, 202, "{}", response.body);
}

fn pending(server: &support::http_server::TestServer) -> Value {
    let response = http_client::request(
        server.addr,
        "GET",
        &format!("/sessions/{CHAT_ID}/pending_tools"),
        None,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    serde_json::from_str(&response.body).expect("pending_tools 应返回 JSON")
}

fn assert_no_pending(server: &support::http_server::TestServer) {
    assert_eq!(pending(server)["pending"], json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn active_host_skill_tool_uses_pending_result_flow_and_deactivation_revokes_it() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(tool_reply(
            "call_before",
            "web_3Asource_2Fread",
            json!({ "path": "before.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("激活前调用已拒绝。")),
        Script::Immediate(tool_reply(
            "call_activate",
            "srv_3Askill_2Factivate",
            json!({ "skill": SKILL }),
        )),
        Script::Immediate(tool_reply(
            "call_source",
            "web_3Asource_2Fread",
            json!({ "path": "src/lib.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("源码读取结果已收到。")),
        Script::Immediate(tool_reply(
            "call_deactivate",
            "srv_3Askill_2Fdeactivate",
            json!({ "skill": SKILL }),
        )),
        Script::Immediate(tool_reply(
            "call_after",
            "web_3Asource_2Fread",
            json!({ "path": "after.rs" }),
        )),
        Script::Immediate(support::wire::text_reply("停用后调用已拒绝。")),
    ]);
    let template = support::http_server::session_template(upstream.endpoint());
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |config| {
            config
                .with_ring_capacity(32)
                .with_cancel_grace(Duration::from_secs(2))
        },
    )
    .await;
    let declaration = json!({
        "id": CHAT_ID,
        "capabilities": { "skills": [{
            "id": SKILL,
            "description": "按需诊断源码",
            "body": "只在激活期间使用源码工具。",
            "tools": [{
                "name": SOURCE_TOOL,
                "description": "读取项目源码文件",
                "schema": { "type": "object", "properties": { "path": { "type": "string" } } },
                "reversibility": "pure"
            }]
        }]}
    });
    let created = http_client::request(
        server.addr,
        "POST",
        "/sessions",
        Some(&declaration.to_string()),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{CHAT_ID}/events"), None);
    assert_eq!(status, 200);

    input(&server, "激活前读取源码");
    let before = drain_until_terminal(&mut sse);
    assert!(before.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == SOURCE_TOOL
    )));
    assert_no_pending(&server);

    input(&server, "激活诊断能力并读取 src/lib.rs");
    let mut activated = false;
    let (executing_id, agent, call_id) = loop {
        let (id, frame) = next_frame(&mut sse);
        activated |= matches!(
            &frame.event,
            SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == ACTIVATE
        );
        if let SessionEvent::ToolExecuting { call_id, request } = frame.event {
            if &*request.tool == SOURCE_TOOL {
                assert_eq!(request.location, Location::Web);
                assert_eq!(request.reversibility, Reversibility::Pure);
                assert_eq!(*request.input, json!({ "path": "src/lib.rs" }));
                break (id, frame.agent, call_id);
            }
        }
    };
    assert!(activated, "源码工具派发前应先成功激活 skill");

    drop(sse);
    let waiting = pending(&server);
    let items = waiting["pending"].as_array().expect("pending 应为数组");
    assert_eq!(items.len(), 1, "断线后必须仍能恢复唯一待办：{waiting}");
    assert_eq!(items[0]["agent"], serde_json::to_value(&agent).unwrap());
    assert_eq!(items[0]["call_id"], serde_json::to_value(&call_id).unwrap());
    assert_eq!(items[0]["request"]["tool"], SOURCE_TOOL);
    assert_eq!(
        items[0]["request"]["input"],
        json!({ "path": "src/lib.rs" })
    );
    assert_eq!(items[0]["request"]["location"], "Web");
    assert_eq!(items[0]["request"]["reversibility"], "Pure");

    let (status, _, mut sse) = http_client::connect_sse(
        server.addr,
        &format!("/sessions/{CHAT_ID}/events"),
        Some(executing_id),
    );
    assert_eq!(status, 200);
    let result = json!({
        "agent": agent,
        "tool_call_id": call_id,
        "result": { "content": "pub fn answer() -> u8 { 42 }" }
    });
    let response = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/tool_result"),
        Some(&result.to_string()),
    );
    assert_eq!(response.status, 202, "{}", response.body);
    let resumed = drain_until_terminal(&mut sse);
    assert!(resumed.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == SOURCE_TOOL
    )));
    assert_no_pending(&server);

    input(&server, "停用诊断能力后再试读");
    let after = drain_until_terminal(&mut sse);
    assert!(after.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: false, .. } if &**tool == DEACTIVATE
    )));
    assert!(after.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == SOURCE_TOOL
    )));
    assert_no_pending(&server);
    assert_eq!(
        upstream.request_count(),
        8,
        "三段对话的 provider 跳数必须完整闭环"
    );
}
