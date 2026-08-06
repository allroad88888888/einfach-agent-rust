use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_core::{AgentId, ExecutionProfileId, Notice, SessionConfig, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_runtime::ExecutionBinding;
use agent_server::{AgentServer, Frame, ServerConfig, SessionEvent, SessionsHandle};
use serde_json::{Value, json};

use super::fixture::{ROOT_KEY, VISION_KEY};
use super::root_script::{handles_from_root_request, selects_first_image};
use crate::image_upload_upstream::{ChatReply, ImageUploadUpstream, UploadReply};
use crate::support;
use crate::support::http_client::{self, SseReader};
use crate::support::server::{FakeServer, Script};

const SESSION_ID: &str = "vision-restart";
const OLD_BYTES: &str = "OLD_IMAGE_RAW_CANARY";

#[tokio::test(flavor = "multi_thread")]
async fn recovered_attachment_handle_fails_closed_before_kimi_io() {
    let vision = ImageUploadUpstream::start_with_chat(
        UploadReply::Ok,
        ChatReply::Text("KIMI_MUST_NOT_RUN".to_string()),
    );
    let root = FakeServer::start(vec![
        Script::Immediate(support::wire::text_reply("PERSISTED_ROOT_FINAL")),
        Script::Dynamic(Arc::new(selects_first_image)),
        Script::Immediate(support::wire::text_reply("RECOVERED_ROOT_FINAL")),
    ]);
    let sessions_dir = support::temp_dir("vision-restart");

    let (first_addr, first_sessions, first_task) =
        start(&root, &vision, sessions_dir.clone()).await;
    create_new(first_addr);
    let first_frames = turn(
        first_addr,
        &json!({
            "text": "remember this image",
            "images": [{
                "name": "old.png",
                "mime": "image/png",
                "bytes": image_bytes()
            }]
        }),
    );
    assert_done(&first_frames, "initial image turn");
    let old_handles = handles_from_root_request(&root.bodies()[0]);
    assert_eq!(
        old_handles.len(),
        1,
        "first root request must expose one handle"
    );
    assert!(
        first_sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
    first_task.abort();

    let (second_addr, second_sessions, second_task) =
        start(&root, &vision, sessions_dir.clone()).await;
    let reopened = http_client::request(
        second_addr,
        "POST",
        "/sessions",
        Some(&json!({"id": SESSION_ID}).to_string()),
    );
    assert_eq!(
        reopened.status, 200,
        "session was not recovered: {}",
        reopened.body
    );
    assert!(reopened.body.contains("recovered"));

    let recovered_frames = turn(second_addr, &json!({"text": "inspect the old image"}));
    assert_done(&recovered_frames, "recovered image turn");
    assert_eq!(root.request_count(), 3, "root must resume exactly once");
    assert_eq!(vision.upload_count(), 0, "unavailable bytes cannot upload");
    assert_eq!(
        vision.chat_count(),
        0,
        "Kimi cannot run without a live lease"
    );

    let root_bodies = root.bodies();
    let recovered_handles = handles_from_root_request(&root_bodies[1]);
    assert!(recovered_handles.contains(&old_handles[0]));
    let outcome = tool_result(&root_bodies[2]);
    assert_eq!(outcome["error"]["code"], "attachment_unavailable");
    assert_eq!(outcome["error"]["retryable"], false);
    assert!(outcome.get("observation").is_none());

    for body in &root_bodies {
        assert_external_surface_is_safe(body);
        assert!(!body.contains("attachment://"));
    }
    let frames = serde_json::to_string(&recovered_frames).unwrap();
    assert_external_surface_is_safe(&frames);
    assert!(!frames.contains("attachment://"));
    let journal = std::fs::read_to_string(sessions_dir.join(format!("{SESSION_ID}.jsonl")))
        .expect("read recovered journal");
    assert!(journal.contains("attachment://"));
    assert_external_surface_is_safe(&journal);

    assert!(
        second_sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
    second_task.abort();
}

async fn start(
    root: &FakeServer,
    vision: &ImageUploadUpstream,
    sessions_dir: PathBuf,
) -> (
    SocketAddr,
    SessionsHandle,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let mut template = support::http_server::session_template(root.endpoint());
    template.api_key = ROOT_KEY.to_string();
    template.default_sessions_dir = Some(sessions_dir);
    let binding = ExecutionBinding::new(
        Arc::new(Kimi),
        Arc::clone(&template.client),
        vision.chat_endpoint(),
        VISION_KEY.to_string(),
        SessionConfig {
            model: Arc::from("kimi-vision-test"),
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
    let task = tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions, task)
}

fn create_new(addr: SocketAddr) {
    let created = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({"id": SESSION_ID}).to_string()),
    );
    assert_eq!(
        created.status, 201,
        "session create failed: {}",
        created.body
    );
}

fn turn(addr: SocketAddr, input: &Value) -> Vec<Frame> {
    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{SESSION_ID}/events"), None);
    let accepted = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{SESSION_ID}/input"),
        Some(&input.to_string()),
    );
    assert_eq!(accepted.status, 202, "input rejected: {}", accepted.body);
    wait_for_terminal(&mut sse)
}

fn wait_for_terminal(sse: &mut SseReader) -> Vec<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data).expect("valid SSE frame");
        let terminal = frame.agent == AgentId::root()
            && matches!(
                &frame.event,
                SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
            );
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
    panic!("timed out waiting for recovered vision turn: {frames:?}");
}

fn assert_done(frames: &[Frame], phase: &str) {
    assert!(
        frames.iter().any(|frame| matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged {
                status: TurnStatus::Done { .. }
            })
        )),
        "{phase} did not finish successfully: {frames:#?}"
    );
}

fn tool_result(body: &str) -> Value {
    let request: Value = serde_json::from_str(body).expect("root request JSON");
    let content = request["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
        })
        .and_then(|message| message["content"].as_str())
        .expect("vision tool result");
    serde_json::from_str(content).expect("vision tool result JSON")
}

fn image_bytes() -> Vec<u8> {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
    bytes.extend_from_slice(OLD_BYTES.as_bytes());
    bytes
}

fn assert_external_surface_is_safe(surface: &str) {
    for forbidden in [
        "ms://",
        "uploaded-image",
        OLD_BYTES,
        ROOT_KEY,
        VISION_KEY,
        "KIMI_MUST_NOT_RUN",
    ] {
        assert!(
            !surface.contains(forbidden),
            "leaked {forbidden}: {surface}"
        );
    }
}
