//! Vision child terminal output is delivered through the parent Tool envelope.

use agent_core::AgentId;
use agent_server::SessionEvent;

use super::fixture::Scenario;
use crate::image_upload_upstream::ChatReply;

const UPLOADED_REFERENCE: &str = "ms://uploaded-image";
const BEFORE: &str = "VISION_BEFORE";
const AFTER: &str = "VISION_AFTER";

#[tokio::test(flavor = "multi_thread")]
async fn vision_child_output_reaches_the_parent_tool_envelope_only() {
    let reply = ChatReply::Chunks(vec![
        format!("{BEFORE} ms://uploaded"),
        format!("-image {AFTER}"),
    ]);
    let scenario = Scenario::run(reply).await;

    assert!(scenario.chat_body().contains(UPLOADED_REFERENCE));
    assert!(scenario.journal.contains(UPLOADED_REFERENCE));
    let root_bodies = scenario.root_bodies();
    assert!(!root_bodies[0].contains(UPLOADED_REFERENCE));
    assert!(root_bodies[1].contains(UPLOADED_REFERENCE));

    let outcome = scenario.root_tool_result();
    assert_eq!(
        outcome["observation"],
        format!("{BEFORE} {UPLOADED_REFERENCE} {AFTER}")
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
