//! Uploaded references remain request-local even when a direct visual provider echoes them.

use std::time::{Duration, Instant};

use agent_core::{ErrorClass, Failure, Notice, TurnStatus};
use agent_server::{Frame, SessionEvent};

use super::{create, start, template};
use crate::image_upload_upstream::{ChatReply, ImageUploadUpstream, UploadReply};
use crate::support;
use crate::support::http_client;

const SESSION_ID: &str = "uploaded-reference-privacy";
const UPLOADED_REFERENCE: &str = "ms://uploaded-image";
const REDACTED_REFERENCE: &str = "[private image reference]";
const BEFORE: &str = "VISUAL_BEFORE";
const AFTER: &str = "VISUAL_AFTER";

#[tokio::test(flavor = "multi_thread")]
async fn direct_visual_success_replays_only_scrubbed_terminal_text() {
    let run = run(ChatReply::Chunks(vec![
        format!("{BEFORE} ms://uploaded"),
        format!("-image {AFTER}"),
    ]))
    .await;

    assert!(matches!(run.status, TurnStatus::Done { .. }));
    assert_request_materialized(&run.upstream);
    assert_public_surfaces_are_scrubbed(&run);
    let frames = serde_json::to_string(&run.frames).expect("serialize public frames");
    assert!(frames.contains(REDACTED_REFERENCE));
    assert!(run.journal.contains(REDACTED_REFERENCE));
    assert!(run.frames.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::TextDelta(text)
            if text.as_ref() == format!("{BEFORE} {REDACTED_REFERENCE} {AFTER}")
    )));
    assert!(run.journal.contains(BEFORE));
    assert!(run.journal.contains(AFTER));
}

#[tokio::test(flavor = "multi_thread")]
async fn direct_visual_http_failure_scrubs_echoed_reference_before_publication() {
    let body = format!(r#"{{"error":"{BEFORE} {UPLOADED_REFERENCE} {AFTER}"}}"#);
    let run = run(ChatReply::StatusBody(400, body)).await;

    assert_eq!(
        run.status,
        TurnStatus::Failed(Failure::Provider(ErrorClass::BadRequest))
    );
    assert_request_materialized(&run.upstream);
    assert_public_surfaces_are_scrubbed(&run);
    assert!(run.frames.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::TransportTrouble(message)
            if message.contains(REDACTED_REFERENCE)
    )));
}

struct Run {
    upstream: ImageUploadUpstream,
    frames: Vec<Frame>,
    journal: String,
    status: TurnStatus,
}

async fn run(chat_reply: ChatReply) -> Run {
    let upstream = ImageUploadUpstream::start_with_chat(UploadReply::Ok, chat_reply);
    let sessions_dir = support::temp_dir(SESSION_ID);
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        sessions_dir.clone(),
    ))
    .await;
    create(addr, SESSION_ID);

    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{SESSION_ID}/events"), None);
    let input = r#"{"text":"inspect","images":[{"name":"image.png","mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#;
    let response = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{SESSION_ID}/input"),
        Some(input),
    );
    assert_eq!(response.status, 202, "input rejected: {}", response.body);
    let (frames, status) = wait_for_terminal(&mut sse);
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
    let journal = std::fs::read_to_string(sessions_dir.join(format!("{SESSION_ID}.jsonl")))
        .expect("read durable visual session");

    Run {
        upstream,
        frames,
        journal,
        status,
    }
}

fn wait_for_terminal(sse: &mut http_client::SseReader) -> (Vec<Frame>, TurnStatus) {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|error| panic!("invalid SSE frame: {error}: {}", event.data));
        let terminal = match &frame.event {
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal() => {
                Some(status.clone())
            }
            _ => None,
        };
        frames.push(frame);
        if let Some(status) = terminal {
            return (frames, status);
        }
    }
    panic!("timed out waiting for visual turn: {frames:?}");
}

fn assert_request_materialized(upstream: &ImageUploadUpstream) {
    let chat = upstream
        .chat_bodies()
        .into_iter()
        .next()
        .expect("one visual chat request");
    assert!(chat.contains(UPLOADED_REFERENCE));
}

fn assert_public_surfaces_are_scrubbed(run: &Run) {
    let frames = serde_json::to_string(&run.frames).expect("serialize public frames");
    for surface in [&frames, &run.journal] {
        assert!(!surface.contains(UPLOADED_REFERENCE), "leaked: {surface}");
    }
}
