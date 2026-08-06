//! Real HTTP/SSE proof for a vision attempt timing out during image upload.

mod upstream;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_core::{AgentId, ExecutionProfileId, Notice, SessionConfig, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_runtime::ExecutionBinding;
use agent_server::{AgentServer, Frame, ServerConfig, SessionEvent, SessionsHandle};
use serde_json::{Value, json};

use self::upstream::GatedVisionUpstream;
use super::fixture::VISION_WIRE_NAME;
use super::root_script::handles_from_root_request;
use crate::support;
use crate::support::http_client::{self, SseReader};
use crate::support::server::{FakeServer, Script};

const SESSION_ID: &str = "vision-timeout";
const ROOT_KEY: &str = "root-timeout-secret";
const VISION_KEY: &str = "vision-timeout-secret";
const RAW_ONE: &str = "TIMEOUT_RAW_ONE";
const RAW_TWO: &str = "TIMEOUT_RAW_TWO";

#[tokio::test(flavor = "multi_thread")]
async fn timed_out_upload_converges_once_and_ignores_its_late_physical_completion() {
    let vision = GatedVisionUpstream::start();
    let root = FakeServer::start(vec![
        Script::Dynamic(Arc::new(selects_both_images)),
        Script::Immediate(support::wire::text_reply("ROOT_AFTER_TIMEOUT")),
        Script::Immediate(support::wire::text_reply("ROOT_FOLLOW_UP")),
    ]);
    let sessions_dir = support::temp_dir("vision-timeout");
    let (addr, sessions) = start_server(&root, &vision, sessions_dir.clone()).await;

    create_session(addr);
    let (status, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{SESSION_ID}/events"), None);
    assert_eq!(status, 200);
    post_images(addr);

    let timeout_frames = wait_for_root_terminal(&mut sse, "timeout parent turn");
    wait_until(
        || vision.uploads_started() == 1,
        "first upload did not start",
    )
    .await;
    assert_eq!(vision.uploads_finished(), 0, "upload must still be blocked");
    assert_eq!(
        vision.chats_started(),
        0,
        "chat must not start after timeout"
    );
    assert_eq!(root.request_count(), 2, "parent must resume exactly once");
    assert_done(&timeout_frames);

    let outcome = root_tool_result(&root.bodies()[1]);
    assert_eq!(outcome["error"]["code"], "vision_timeout");
    assert_eq!(outcome["error"]["retryable"], true);
    assert!(outcome.get("observation").is_none());

    // A later turn exercises the session again while the old blocking HTTP worker is still alive.
    // Its success must not clear the timed-out call's own monotonic cancellation latch.
    post_text(addr, "a new parent turn");
    let follow_up_frames = wait_for_root_terminal(&mut sse, "follow-up parent turn");
    assert_done(&follow_up_frames);
    assert_eq!(
        root.request_count(),
        3,
        "follow-up should make one root call"
    );
    assert_eq!(
        vision.uploads_started(),
        1,
        "no later image upload may start"
    );
    assert_eq!(vision.chats_started(), 0, "no vision chat may start");

    let journal_path = sessions_dir.join(format!("{SESSION_ID}.jsonl"));
    let before_late = std::fs::read_to_string(&journal_path).expect("read converged journal");
    vision.release_upload();
    wait_until(
        || vision.uploads_finished() == 1,
        "blocked upstream request did not finish after release",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // This observes a server-side response attempt, not guaranteed physical cancellation or abort.
    assert_eq!(
        vision.uploads_started(),
        1,
        "late result started another upload"
    );
    assert_eq!(
        vision.chats_started(),
        0,
        "late upload result started vision chat"
    );
    assert_eq!(
        root.request_count(),
        3,
        "late result resumed the parent again"
    );
    assert!(
        sse.next_event(Duration::from_millis(250)).is_none(),
        "late completion emitted another public frame"
    );
    let after_late = std::fs::read_to_string(&journal_path).expect("read journal after late I/O");
    assert_eq!(
        after_late, before_late,
        "late completion mutated durable state"
    );

    assert_safe(&root.bodies().join("\n"));
    assert_safe(&serde_json::to_string(&timeout_frames).unwrap());
    assert_safe(&after_late);
    assert!(after_late.contains("vision_timeout"));
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

async fn start_server(
    root: &FakeServer,
    vision: &GatedVisionUpstream,
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
            model: Arc::from("kimi-vision-timeout"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
    )
    .with_timeout(Duration::from_millis(500))
    .with_image_upload_base_url(vision.upload_base_url());
    let bindings = BTreeMap::from([(ExecutionProfileId::new("vision"), binding)]);
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY)
            .with_execution_bindings(bindings),
    );
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn create_session(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({"id": SESSION_ID}).to_string()),
    );
    assert_eq!(response.status, 201, "create failed: {}", response.body);
}

fn post_images(addr: SocketAddr) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{SESSION_ID}/input"),
        Some(
            &json!({
                "text": "inspect both images",
                "images": [
                    {"name":"one.png", "mime":"image/png", "bytes":image_bytes(RAW_ONE)},
                    {"name":"two.png", "mime":"image/png", "bytes":image_bytes(RAW_TWO)}
                ]
            })
            .to_string(),
        ),
    );
    assert_eq!(
        response.status, 202,
        "image input rejected: {}",
        response.body
    );
}

fn post_text(addr: SocketAddr, text: &str) {
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{SESSION_ID}/input"),
        Some(&json!({"text": text}).to_string()),
    );
    assert_eq!(
        response.status, 202,
        "text input rejected: {}",
        response.body
    );
}

fn selects_both_images(body: &str) -> String {
    let handles = handles_from_root_request(body);
    assert_eq!(handles.len(), 2, "root must receive both image handles");
    let arguments = json!({"images": handles, "question": "inspect both"}).to_string();
    let tool = json!({
        "choices":[{"index":0,"delta":{"role":"assistant","content":Value::Null,
            "tool_calls":[{"index":0,"id":"call_vision_timeout","type":"function",
                "function":{"name":VISION_WIRE_NAME,"arguments":arguments}}]},
            "finish_reason":Value::Null}]
    });
    let finish = json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]});
    format!("data: {tool}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn root_tool_result(body: &str) -> Value {
    let request: Value = serde_json::from_str(body).expect("second root request JSON");
    let content = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .expect("timeout Tool result");
    serde_json::from_str(content).expect("timeout Tool result JSON")
}

fn wait_for_root_terminal(sse: &mut SseReader, label: &str) -> Vec<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let event = sse
            .next_event(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|| panic!("timed out waiting for {label}: {frames:?}"));
        let frame: Frame = serde_json::from_str(&event.data).expect("valid public Frame");
        let terminal = frame.agent == AgentId::root()
            && matches!(&frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal());
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
    panic!("timed out waiting for {label}: {frames:?}");
}

fn assert_done(frames: &[Frame]) {
    assert!(frames.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::Notice(Notice::TurnStatusChanged {
            status: TurnStatus::Done { .. }
        })
    )));
}

async fn wait_until(mut condition: impl FnMut() -> bool, failure: &str) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(condition(), "{failure}");
}

fn image_bytes(canary: &str) -> Vec<u8> {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
    bytes.extend_from_slice(canary.as_bytes());
    bytes
}

fn assert_safe(surface: &str) {
    for forbidden in [
        "ms://",
        "late-upload-reference",
        ROOT_KEY,
        VISION_KEY,
        RAW_ONE,
        RAW_TWO,
    ] {
        assert!(
            !surface.contains(forbidden),
            "leaked {forbidden}: {surface}"
        );
    }
}
