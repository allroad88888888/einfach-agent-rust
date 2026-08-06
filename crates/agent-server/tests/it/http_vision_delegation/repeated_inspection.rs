//! Two independent vision inspections in one durable parent session.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_core::{AgentId, ExecutionProfileId, Notice, SessionConfig, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_runtime::ExecutionBinding;
use agent_server::{AgentServer, Frame, ServerConfig, SessionEvent, SessionsHandle};
use serde_json::{Value, json};

use super::fixture::{OBSERVATION, ROOT_KEY, VISION_KEY, VISION_WIRE_NAME};
use super::root_script::handles_from_root_request;
use crate::image_upload_upstream::{ChatReply, ImageUploadUpstream, UploadReply};
use crate::support;
use crate::support::http_client::{self, SseReader};
use crate::support::server::{FakeServer, Script};

const SESSION_ID: &str = "vision-repeated-inspection";
const FIRST_BYTES: &str = "FIRST_INSPECTION_RAW";
const SECOND_BYTES: &str = "SECOND_INSPECTION_RAW";
const FIRST_QUESTION: &str = "FIRST_IMAGE_QUESTION";
const SECOND_QUESTION: &str = "SECOND_IMAGE_QUESTION";

#[tokio::test(flavor = "multi_thread")]
async fn sequential_inspections_materialize_distinct_handles_without_cross_talk() {
    let vision = ImageUploadUpstream::start_with_chat(
        UploadReply::Ok,
        ChatReply::Text(OBSERVATION.to_string()),
    );
    let root = FakeServer::start(vec![
        Script::Dynamic(Arc::new(selects_first_turn_image)),
        Script::Immediate(support::wire::text_reply("FIRST_PARENT_DONE")),
        Script::Dynamic(Arc::new(selects_second_turn_image)),
        Script::Immediate(support::wire::text_reply("SECOND_PARENT_DONE")),
    ]);
    let sessions_dir = support::temp_dir("vision-repeated-inspection");
    let (addr, sessions) = start(&root, &vision, sessions_dir.clone()).await;

    create(addr);
    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{SESSION_ID}/events"), None);
    post_image(addr, "first.png", FIRST_BYTES);
    let first_frames = wait_for_done(&mut sse, "first inspection");
    post_image(addr, "second.png", SECOND_BYTES);
    let second_frames = wait_for_done(&mut sse, "second inspection");

    assert_eq!(root.request_count(), 4, "each Tool call must resume once");
    assert_eq!(
        vision.upload_count(),
        2,
        "one materialization per Tool call"
    );
    assert_eq!(vision.chat_count(), 2, "one Kimi call per Tool call");

    let root_bodies = root.bodies();
    let first_handles = handles_from_root_request(&root_bodies[0]);
    let second_handles = handles_from_root_request(&root_bodies[2]);
    assert_eq!(first_handles.len(), 1, "first turn must expose one handle");
    assert_eq!(
        second_handles.len(),
        2,
        "second turn must expose both handles"
    );
    assert_ne!(first_handles[0], second_handles[1]);

    let uploads: Vec<_> = vision
        .calls()
        .into_iter()
        .filter(|call| call.path.ends_with("/files"))
        .map(|call| call.body)
        .collect();
    assert_materialized_only(&uploads[0], FIRST_BYTES, SECOND_BYTES);
    assert_materialized_only(&uploads[1], SECOND_BYTES, FIRST_BYTES);

    let chats = vision.chat_bodies();
    assert_child_question(&chats[0], FIRST_QUESTION);
    assert_child_question(&chats[1], SECOND_QUESTION);
    assert_eq!(tool_result(&root_bodies[1])["observation"], OBSERVATION);
    assert_eq!(tool_result(&root_bodies[3])["observation"], OBSERVATION);
    assert_one_done(&first_frames, "first inspection");
    assert_one_done(&second_frames, "second inspection");

    let journal = std::fs::read_to_string(sessions_dir.join(format!("{SESSION_ID}.jsonl")))
        .expect("read repeated-inspection journal");
    for external in [root_bodies.join("\n"), journal] {
        assert!(!external.contains(FIRST_BYTES));
        assert!(!external.contains(SECOND_BYTES));
        assert!(!external.contains(VISION_KEY));
        assert!(!external.contains("ms://"));
    }
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

fn selects_first_turn_image(body: &str) -> String {
    select(body, 0, FIRST_QUESTION, "call_vision_first")
}

fn selects_second_turn_image(body: &str) -> String {
    select(body, 1, SECOND_QUESTION, "call_vision_second")
}

fn select(body: &str, index: usize, question: &str, call_id: &str) -> String {
    let handles = handles_from_root_request(body);
    let selected = handles
        .get(index)
        .unwrap_or_else(|| panic!("missing handle {index} in {handles:?}"));
    let arguments = json!({"images": [selected], "question": question}).to_string();
    let tool = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {"name": VISION_WIRE_NAME, "arguments": arguments}
                }]
            },
            "finish_reason": Value::Null
        }]
    });
    let finish = json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {tool}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

async fn start(
    root: &FakeServer,
    vision: &ImageUploadUpstream,
    sessions_dir: std::path::PathBuf,
) -> (SocketAddr, SessionsHandle) {
    let mut template = support::http_server::session_template(root.endpoint());
    template.api_key = ROOT_KEY.to_string();
    template.default_sessions_dir = Some(sessions_dir);
    let binding = ExecutionBinding::new(
        Arc::new(Kimi),
        Arc::clone(&template.client),
        vision.chat_endpoint(),
        VISION_KEY.to_string(),
        SessionConfig {
            model: Arc::from("kimi-vision-repeated"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
    )
    .with_timeout(Duration::from_secs(5))
    .with_image_upload_base_url(vision.upload_base_url());
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY)
            .with_execution_bindings(BTreeMap::from([(
                ExecutionProfileId::new("vision"),
                binding,
            )])),
    );
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn create(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({"id": SESSION_ID}).to_string()),
    );
    assert_eq!(response.status, 201, "create failed: {}", response.body);
}

fn post_image(addr: SocketAddr, name: &str, canary: &str) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{SESSION_ID}/input"),
        Some(
            &json!({
                "text": format!("inspect {name}"),
                "images": [{
                    "name": name,
                    "mime": "image/png",
                    "bytes": image_bytes(canary)
                }]
            })
            .to_string(),
        ),
    );
    assert_eq!(
        response.status, 202,
        "image input failed: {}",
        response.body
    );
}

fn wait_for_done(sse: &mut SseReader, label: &str) -> Vec<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let event = sse
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| panic!("timed out waiting for {label}: {frames:?}"));
        let frame: Frame = serde_json::from_str(&event.data).expect("valid public Frame");
        let done = frame.agent == AgentId::root()
            && matches!(
                frame.event,
                SessionEvent::Notice(Notice::TurnStatusChanged {
                    status: TurnStatus::Done { .. }
                })
            );
        frames.push(frame);
        if done {
            return frames;
        }
    }
    panic!("timed out waiting for {label}: {frames:?}");
}

fn tool_result(body: &str) -> Value {
    let request: Value = serde_json::from_str(body).expect("resumed root request JSON");
    let content = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .expect("vision Tool result");
    serde_json::from_str(content).expect("vision Tool result JSON")
}

fn assert_materialized_only(body: &str, selected: &str, other: &str) {
    assert!(body.contains(selected), "selected bytes missing: {body}");
    assert!(!body.contains(other), "other image crossed calls: {body}");
}

fn assert_child_question(body: &str, expected: &str) {
    let request: Value = serde_json::from_str(body).expect("Kimi request JSON");
    let content = request["messages"]
        .as_array()
        .expect("Kimi messages")
        .iter()
        .find(|message| message["role"] == "user")
        .map(|message| &message["content"])
        .expect("one Kimi user message");
    assert_eq!(content[0], json!({"type":"text", "text": expected}));
    assert_eq!(content[1]["image_url"]["url"], "ms://uploaded-image");
}

fn assert_one_done(frames: &[Frame], label: &str) {
    let done = frames
        .iter()
        .filter(|frame| {
            frame.agent == AgentId::root()
                && matches!(
                    frame.event,
                    SessionEvent::Notice(Notice::TurnStatusChanged {
                        status: TurnStatus::Done { .. }
                    })
                )
        })
        .count();
    assert_eq!(done, 1, "{label} must converge exactly once");
}

fn image_bytes(canary: &str) -> Vec<u8> {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
    bytes.extend_from_slice(canary.as_bytes());
    bytes
}
