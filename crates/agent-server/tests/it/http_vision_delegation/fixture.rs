use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_core::{AgentId, ExecutionProfileId, Notice, SessionConfig, SystemChunk};
use agent_providers::kimi::Kimi;
use agent_runtime::ExecutionBinding;
use agent_server::{AgentServer, Frame, ServerConfig, SessionEvent, SessionsHandle};
use serde_json::{Value, json};

use crate::image_upload_upstream::{ChatReply, ImageUploadUpstream, UploadReply};
use crate::support;
use crate::support::http_client::{self, SseReader};
use crate::support::server::{FakeServer, Script};

use super::root_script::{handles_from_root_request, selects_second_image};

pub const OBSERVATION: &str = "SELECTED_IMAGE_OBSERVATION";
pub const HOST_SYSTEM_CANARY: &str = "HOST_SYSTEM_CONTEXT_CANARY";
pub const PARENT_CANARY: &str = "PARENT_HISTORY_CANARY";
pub const QUESTION: &str = "SECOND_IMAGE_QUESTION";
pub const ROOT_KEY: &str = "root-secret-key";
pub const VISION_KEY: &str = "vision-secret-key";
pub const VISION_WIRE_NAME: &str = "srv_3Avision_2Finspect";

const SESSION_ID: &str = "vision-delegation";
const SELECTED_BYTES_CANARY: &str = "SELECTED_RAW_BYTES_CANARY";
const UNSELECTED_BYTES_CANARY: &str = "UNSELECTED_RAW_BYTES_CANARY";

pub struct Scenario {
    pub root: FakeServer,
    pub vision: ImageUploadUpstream,
    pub frames: Vec<Frame>,
    pub journal: String,
    pub handles: Vec<String>,
    raw_byte_wires: [String; 2],
}

impl Scenario {
    pub async fn run(chat_reply: ChatReply) -> Self {
        let vision = ImageUploadUpstream::start_with_chat(UploadReply::Ok, chat_reply);
        let root = FakeServer::start(vec![
            Script::Dynamic(Arc::new(selects_second_image)),
            Script::Immediate(support::wire::text_reply("ROOT_FINAL")),
        ]);
        let sessions_dir = support::temp_dir("vision-delegation");
        let mut template = support::http_server::session_template(root.endpoint());
        template.api_key = ROOT_KEY.to_string();
        template.default_sessions_dir = Some(sessions_dir.clone());
        template.system = vec![SystemChunk {
            label: Arc::from("host-private-context"),
            text: Arc::from(HOST_SYSTEM_CANARY),
        }];

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
        let bindings = BTreeMap::from([(ExecutionProfileId::new("vision"), binding)]);
        let (addr, sessions) = start(template, bindings).await;

        create(addr);
        let (_, _, mut sse) =
            http_client::connect_sse(addr, &format!("/sessions/{SESSION_ID}/events"), None);
        let first_image = image_bytes(UNSELECTED_BYTES_CANARY);
        let second_image = image_bytes(SELECTED_BYTES_CANARY);
        let raw_byte_wires = [
            serde_json::to_string(&first_image).unwrap(),
            serde_json::to_string(&second_image).unwrap(),
        ];
        let input = json!({
            "text": format!("inspect one image; private context: {PARENT_CANARY}"),
            "images": [
                {"name": "unselected.png", "mime": "image/png", "bytes": first_image},
                {"name": "selected.png", "mime": "image/png", "bytes": second_image}
            ]
        });
        let accepted = http_client::request(
            addr,
            "POST",
            &format!("/sessions/{SESSION_ID}/input"),
            Some(&input.to_string()),
        );
        assert_eq!(accepted.status, 202, "input rejected: {}", accepted.body);
        let frames = wait_for_terminal(&mut sse);
        assert!(
            sessions
                .close_all()
                .iter()
                .all(|(_, result)| result.is_ok()),
            "vision test session must close cleanly"
        );
        let journal = std::fs::read_to_string(sessions_dir.join(format!("{SESSION_ID}.jsonl")))
            .expect("read durable vision session");
        let root_bodies = root.bodies();
        let handles = root_bodies
            .first()
            .map(|body| handles_from_root_request(body))
            .unwrap_or_default();

        Self {
            root,
            vision,
            frames,
            journal,
            handles,
            raw_byte_wires,
        }
    }

    pub fn root_bodies(&self) -> Vec<String> {
        self.root.bodies()
    }

    pub fn chat_body(&self) -> String {
        self.vision
            .chat_bodies()
            .into_iter()
            .next()
            .expect("one Kimi chat request")
    }

    pub fn upload_body(&self) -> String {
        self.vision
            .calls()
            .into_iter()
            .find(|call| call.path.ends_with("/files"))
            .expect("one Kimi upload request")
            .body
    }

    pub fn root_tool_result(&self) -> Value {
        let bodies = self.root_bodies();
        let request: Value = serde_json::from_str(&bodies[1]).expect("second DeepSeek request");
        let content = request["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .rev()
                    .find(|message| message["role"] == "tool")
            })
            .and_then(|message| message["content"].as_str())
            .expect("vision tool result in second DeepSeek request");
        serde_json::from_str(content).expect("vision tool result JSON")
    }

    pub fn assert_provider_material_absent(&self, surface: &str) {
        for secret in [
            "ms://",
            "uploaded-image",
            ROOT_KEY,
            VISION_KEY,
            "vision-upstream-secret",
            SELECTED_BYTES_CANARY,
            UNSELECTED_BYTES_CANARY,
        ] {
            assert!(!surface.contains(secret), "leaked {secret}: {surface}");
        }
        for raw in &self.raw_byte_wires {
            assert!(
                !surface.contains(raw),
                "raw attachment bytes leaked: {surface}"
            );
        }
    }
}

fn image_bytes(canary: &str) -> Vec<u8> {
    let mut bytes = vec![137, 80, 78, 71, 13, 10, 26, 10];
    bytes.extend_from_slice(canary.as_bytes());
    bytes
}

async fn start(
    template: agent_server::SessionTemplate,
    bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY)
            .with_execution_bindings(bindings),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind vision delegation server");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn create(addr: SocketAddr) {
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

fn wait_for_terminal(sse: &mut SseReader) -> Vec<Frame> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|error| panic!("invalid SSE Frame: {error}: {}", event.data));
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
    panic!("timed out waiting for vision turn: {frames:?}");
}
