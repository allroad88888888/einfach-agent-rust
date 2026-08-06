use std::sync::Arc;

use agent_core::{ContentBlock, StopReason, ToolCallId};
use serde_json::json;

use super::{REDACTED_IMAGE_REFERENCE, scrub_terminal};

#[test]
fn exact_upload_references_are_removed_from_every_terminal_string_surface() {
    let short: Arc<str> = Arc::from("ms://uploaded");
    let long: Arc<str> = Arc::from("ms://uploaded/second");
    let mut blocks = vec![
        ContentBlock::Text(Arc::from("saw ms://uploaded/second and ms://uploaded")),
        ContentBlock::Thinking(Arc::from("think ms://uploaded")),
        ContentBlock::ToolUse {
            id: ToolCallId::new("call-ms://uploaded"),
            name: Arc::from("inspect-ms://uploaded/second"),
            input: Arc::new(json!({
                "ms://uploaded": [
                    {"nested": "before ms://uploaded/second after"},
                    "ms://unrelated"
                ]
            })),
        },
        ContentBlock::ToolResult {
            id: ToolCallId::new("result-ms://uploaded"),
            content: Arc::from("result ms://uploaded/second"),
            is_error: false,
        },
        ContentBlock::Image {
            reference: Arc::from("ms://uploaded"),
            mime: Arc::from("image/ms://uploaded/second"),
            name: Some(Arc::from("name-ms://uploaded")),
        },
    ];
    let mut stop = StopReason::Other(Arc::from("finished-ms://uploaded/second"));

    scrub_terminal(
        &mut blocks,
        &mut stop,
        &[Arc::clone(&short), Arc::clone(&long), Arc::clone(&short)],
    );

    let wire = serde_json::to_string(&(blocks, stop)).expect("serialize scrubbed terminal output");
    assert!(!wire.contains(short.as_ref()));
    assert!(!wire.contains(long.as_ref()));
    assert!(wire.contains(REDACTED_IMAGE_REFERENCE));
    assert!(wire.contains("ms://unrelated"));
}

#[test]
fn absent_reference_set_does_not_guess_provider_tokens() {
    let original = vec![ContentBlock::Text(Arc::from(
        "ms://not-supplied endpoint model api-key-shaped-text",
    ))];
    let mut blocks = original.clone();
    let original_stop = StopReason::Other(Arc::from("ms://not-supplied"));
    let mut stop = original_stop.clone();

    scrub_terminal(&mut blocks, &mut stop, &[Arc::from("")]);

    assert_eq!(blocks, original);
    assert_eq!(stop, original_stop);
}
