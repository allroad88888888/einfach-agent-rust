use agent_core::{Notice, TurnStatus};
use agent_server::SessionEvent;

use super::fixture::{OBSERVATION, Scenario};
use crate::image_upload_upstream::ChatReply;

#[tokio::test(flavor = "multi_thread")]
async fn retryable_kimi_failure_returns_one_sanitized_child_outcome_without_retrying() {
    assert_child_failure(ChatReply::Status(503), true).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_kimi_completion_returns_one_non_retryable_child_outcome() {
    assert_child_failure(ChatReply::Empty, false).await;
}

async fn assert_child_failure(reply: ChatReply, retryable: bool) {
    let scenario = Scenario::run(reply).await;

    assert_eq!(scenario.root.request_count(), 2, "root must resume once");
    assert_eq!(
        scenario.vision.upload_count(),
        1,
        "selected image uploads once"
    );
    assert_eq!(
        scenario.vision.chat_count(),
        1,
        "vision child max_retries=0 must suppress provider retries"
    );

    let outcome = scenario.root_tool_result();
    assert_eq!(outcome["error"]["code"], "vision_child_failed");
    assert_eq!(outcome["error"]["retryable"], retryable);
    assert!(outcome.get("observation").is_none());

    for body in scenario.root_bodies() {
        scenario.assert_provider_material_absent(&body);
        assert!(!body.contains("attachment://"));
    }
    assert!(!scenario.root_bodies()[1].contains(OBSERVATION));
    scenario.assert_provider_material_absent(&scenario.journal);
    assert!(scenario.journal.contains("attachment://"));
    let frames = serde_json::to_string(&scenario.frames).expect("serialize SSE frames");
    scenario.assert_provider_material_absent(&frames);
    assert!(!frames.contains("attachment://"));
    assert!(scenario.frames.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::Notice(Notice::TurnStatusChanged {
            status: TurnStatus::Done { .. }
        })
    )));
}
