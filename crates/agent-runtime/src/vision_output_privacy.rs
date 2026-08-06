//! Exact-value privacy gate for provider output that may echo request-local image references.

use std::sync::Arc;

use agent_core::{ContentBlock, StopReason};
use serde_json::Value;

const REDACTED_IMAGE_REFERENCE: &str = "[private image reference]";

/// Remove request-local upload references before provider blocks can enter core state.
///
/// The reference set comes directly from this request's successful uploads. This deliberately
/// does not infer secrets from prefixes or token shapes.
pub(crate) fn scrub_terminal(
    blocks: &mut [ContentBlock],
    stop: &mut StopReason,
    references: &[Arc<str>],
) {
    let references = normalized(references);
    if references.is_empty() {
        return;
    }
    for block in blocks {
        scrub_block(block, &references);
    }
    if let StopReason::Other(reason) = stop {
        scrub_arc(reason, &references);
    }
}

/// Remove only exact references issued for this request from one provider-controlled string.
pub(crate) fn scrub_text(value: &str, references: &[Arc<str>]) -> String {
    scrub_string(value, &normalized(references))
}

fn normalized(references: &[Arc<str>]) -> Vec<&str> {
    let mut references: Vec<_> = references
        .iter()
        .map(AsRef::as_ref)
        .filter(|reference: &&str| !reference.is_empty())
        .collect();
    references
        .sort_unstable_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    references.dedup();
    references
}

fn scrub_block(block: &mut ContentBlock, references: &[&str]) {
    match block {
        ContentBlock::Text(text) | ContentBlock::Thinking(text) => scrub_arc(text, references),
        ContentBlock::ToolUse { id, name, input } => {
            scrub_arc(&mut id.0, references);
            scrub_arc(name, references);
            let mut value = input.as_ref().clone();
            scrub_json(&mut value, references);
            *input = Arc::new(value);
        }
        ContentBlock::ToolResult { id, content, .. } => {
            scrub_arc(&mut id.0, references);
            scrub_arc(content, references);
        }
        ContentBlock::Image {
            reference,
            mime,
            name,
        } => {
            scrub_arc(reference, references);
            scrub_arc(mime, references);
            if let Some(name) = name {
                scrub_arc(name, references);
            }
        }
    }
}

fn scrub_arc(value: &mut Arc<str>, references: &[&str]) {
    let scrubbed = scrub_string(value, references);
    if scrubbed != value.as_ref() {
        *value = Arc::from(scrubbed);
    }
}

fn scrub_string(value: &str, references: &[&str]) -> String {
    references.iter().fold(value.to_owned(), |text, reference| {
        text.replace(reference, REDACTED_IMAGE_REFERENCE)
    })
}

fn scrub_json(value: &mut Value, references: &[&str]) {
    match value {
        Value::String(text) => *text = scrub_string(text, references),
        Value::Array(values) => {
            for value in values {
                scrub_json(value, references);
            }
        }
        Value::Object(values) => {
            let original = std::mem::take(values);
            for (key, mut value) in original {
                scrub_json(&mut value, references);
                values.insert(scrub_string(&key, references), value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
#[path = "vision_output_privacy_tests.rs"]
mod tests;
