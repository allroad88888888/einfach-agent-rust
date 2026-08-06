use agent_core::{Notice, TurnStatus};
use agent_server::SessionEvent;
use serde_json::{Value, json};

use super::fixture::{
    HOST_SYSTEM_CANARY, OBSERVATION, PARENT_CANARY, QUESTION, ROOT_KEY, Scenario, VISION_KEY,
    VISION_WIRE_NAME,
};
use crate::image_upload_upstream::ChatReply;

#[tokio::test(flavor = "multi_thread")]
async fn deepseek_delegates_only_the_selected_image_to_an_isolated_kimi_child() {
    let scenario = Scenario::run(ChatReply::Text(OBSERVATION.to_string())).await;

    assert_eq!(scenario.root.request_count(), 2, "root must resume once");
    assert_eq!(
        scenario.vision.upload_count(),
        1,
        "only one image is selected"
    );
    assert_eq!(scenario.vision.chat_count(), 1, "one child provider call");

    let root_bodies = scenario.root_bodies();
    assert!(root_bodies[0].contains(VISION_WIRE_NAME));
    assert_eq!(
        scenario.handles.len(),
        2,
        "root must see both opaque handles"
    );
    assert_ne!(scenario.handles[0], scenario.handles[1]);
    for body in &root_bodies {
        scenario.assert_provider_material_absent(body);
        assert!(
            !body.contains("attachment://"),
            "root saw an internal URI: {body}"
        );
    }

    assert_selected_upload_only(&scenario.upload_body());
    assert_isolated_child_request(&scenario.chat_body());

    let outcome = scenario.root_tool_result();
    assert_eq!(outcome["observation"], OBSERVATION);
    assert_eq!(outcome["metadata"]["images_inspected"], 1);
    assert_eq!(outcome["metadata"]["truncated"], false);
    assert!(outcome.get("error").is_none());
    assert!(root_bodies[1].contains(OBSERVATION));
    assert!(!root_bodies[1].contains("vision_child_failed"));

    scenario.assert_provider_material_absent(&scenario.journal);
    assert!(
        scenario.journal.contains("attachment://"),
        "durable history should keep only provider-neutral attachment refs"
    );
    let frames = serde_json::to_string(&scenario.frames).expect("serialize SSE frames");
    scenario.assert_provider_material_absent(&frames);
    assert!(
        !frames.contains("attachment://"),
        "SSE exposed an internal attachment URI: {frames}"
    );
    assert!(scenario.frames.iter().any(|frame| matches!(
        &frame.event,
        SessionEvent::Notice(Notice::TurnStatusChanged {
            status: TurnStatus::Done { .. }
        })
    )));
}

fn assert_selected_upload_only(body: &str) {
    assert!(
        body.contains("selected.png"),
        "selected name missing: {body}"
    );
    assert!(
        body.contains("SELECTED_RAW_BYTES_CANARY"),
        "selected bytes missing: {body}"
    );
    assert!(
        !body.contains("unselected.png"),
        "wrong image uploaded: {body}"
    );
    assert!(
        !body.contains("UNSELECTED_RAW_BYTES_CANARY"),
        "unselected bytes uploaded: {body}"
    );
}

fn assert_isolated_child_request(body: &str) {
    let request: Value = serde_json::from_str(body).expect("Kimi child request JSON");
    let messages = request["messages"].as_array().expect("Kimi messages");
    let users: Vec<_> = messages
        .iter()
        .filter(|message| message["role"] == "user")
        .collect();
    assert_eq!(users.len(), 1, "child inherited parent users: {request}");
    assert!(
        messages
            .iter()
            .all(|message| { message["role"] != "assistant" && message["role"] != "tool" })
    );
    assert_eq!(
        users[0]["content"],
        json!([
            {"type":"text", "text": QUESTION},
            {"type":"image_url", "image_url":{"url":"ms://uploaded-image"}}
        ])
    );
    assert!(
        request
            .get("tools")
            .is_none_or(|tools| tools.as_array().is_some_and(Vec::is_empty)),
        "vision child must have no tools: {request}"
    );
    assert_eq!(body.matches("ms://uploaded-image").count(), 1);
    assert!(!body.contains("attachment://"));
    for forbidden in [
        PARENT_CANARY,
        HOST_SYSTEM_CANARY,
        "unselected.png",
        "UNSELECTED_RAW_BYTES_CANARY",
        "SELECTED_RAW_BYTES_CANARY",
        ROOT_KEY,
        VISION_KEY,
    ] {
        assert!(
            !body.contains(forbidden),
            "child leaked {forbidden}: {body}"
        );
    }
}
