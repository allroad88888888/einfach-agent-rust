//! Vision child provider output crosses only the scrubbed parent Tool envelope.

use agent_core::AgentId;
use agent_server::SessionEvent;

use super::fixture::Scenario;
use crate::image_upload_upstream::ChatReply;

const UPLOADED_REFERENCE: &str = "ms://uploaded-image";
const REDACTED_REFERENCE: &str = "[private image reference]";
const BEFORE: &str = "VISION_BEFORE";
const AFTER: &str = "VISION_AFTER";

#[tokio::test(flavor = "multi_thread")]
async fn malicious_vision_echo_is_scrubbed_before_every_public_or_durable_surface() {
    let reply = ChatReply::Chunks(vec![
        format!("{BEFORE} ms://uploaded"),
        format!("-image {AFTER}"),
    ]);
    let scenario = Scenario::run(reply).await;

    assert!(scenario.chat_body().contains(UPLOADED_REFERENCE));
    let frames = serde_json::to_string(&scenario.frames).expect("serialize vision frames");
    scenario.assert_provider_material_absent(&frames);
    scenario.assert_provider_material_absent(&scenario.journal);
    for body in scenario.root_bodies() {
        scenario.assert_provider_material_absent(&body);
    }

    let outcome = scenario.root_tool_result();
    assert_eq!(
        outcome["observation"],
        format!("{BEFORE} {REDACTED_REFERENCE} {AFTER}")
    );
    assert!(scenario.frames.iter().all(|frame| {
        frame.agent == AgentId::root()
            || !matches!(
                frame.event,
                SessionEvent::TextDelta(_)
                    | SessionEvent::ThinkingDelta(_)
                    | SessionEvent::ToolCallStarted { .. }
            )
    }));
}
