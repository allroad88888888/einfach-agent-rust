//! 视觉工具 schema 与严格 parser 的单元面。

use serde_json::json;

use super::*;
use crate::vision::VisionFailureCode;

fn assert_invalid(input: Value) {
    let error = parse_vision_inspect_request(&input).unwrap_err();
    assert_eq!(error.code(), VisionFailureCode::InvalidInput);
    assert!(!error.retryable());
}

#[test]
fn parses_bare_handles_and_normalizes_internal_references() {
    let request = parse_vision_inspect_request(&json!({
        "images": ["img_1", "img_42"],
        "question": "  Read the visible error.  "
    }))
    .unwrap();

    assert_eq!(request.question(), "Read the visible error.");
    assert_eq!(request.images()[0].as_str(), "img_1");
    assert_eq!(
        &*request.images()[1].attachment_reference(),
        "attachment://img_42"
    );
}

#[test]
fn rejects_missing_empty_oversized_or_duplicate_images() {
    assert_invalid(json!({"question": "q"}));
    assert_invalid(json!({"images": [], "question": "q"}));
    assert_invalid(json!({
        "images": (1..=MAX_VISION_IMAGES + 1)
            .map(|number| format!("img_{number}"))
            .collect::<Vec<_>>(),
        "question": "q"
    }));
    assert_invalid(json!({"images": ["img_1", "img_1"], "question": "q"}));
}

#[test]
fn rejects_external_provider_and_malformed_references_without_echoing_them() {
    for bad in [
        "https://example.test/private-token",
        "file:///tmp/secret.png",
        "ms://provider-secret",
        "attachment://img_1",
    ] {
        let error = parse_vision_inspect_request(&json!({
            "images": [bad],
            "question": "q"
        }))
        .unwrap_err();
        assert_eq!(error.code(), VisionFailureCode::InvalidInput);
        assert!(!error.message().contains(bad));
    }

    for bad in [
        "img_",
        "img_0",
        "img_01",
        "img_12x",
        "IMG_12",
        "img_18446744073709551615",
        "img_18446744073709551616",
    ] {
        let error = parse_vision_inspect_request(&json!({
            "images": [bad],
            "question": "q"
        }))
        .unwrap_err();
        assert_eq!(error.code(), VisionFailureCode::InvalidInput);
    }
}

#[test]
fn question_is_required_nonblank_bounded_and_a_string() {
    assert_invalid(json!({"images": ["img_1"]}));
    assert_invalid(json!({"images": ["img_1"], "question": " \n\t "}));
    assert_invalid(json!({"images": ["img_1"], "question": 7}));
    assert_invalid(json!({
        "images": ["img_1"],
        "question": "问".repeat(MAX_VISION_QUESTION_CHARS + 1)
    }));
    assert_invalid(json!({
        "images": ["img_1"],
        "question": format!("{}q", " ".repeat(MAX_VISION_QUESTION_CHARS))
    }));
}

#[test]
fn parser_and_schema_reject_unknown_fields() {
    assert_invalid(json!({
        "images": ["img_1"],
        "question": "q",
        "provider": "forbidden-route"
    }));
    let spec = vision_inspect_spec();
    assert_eq!(&*spec.name, VISION_INSPECT_TOOL);
    assert_eq!(spec.schema["additionalProperties"], false);
    assert_eq!(spec.schema["properties"]["images"]["minItems"], 1);
    assert_eq!(
        spec.schema["properties"]["images"]["items"]["pattern"],
        "^img_[1-9][0-9]*$"
    );
    assert_eq!(
        spec.schema["properties"]["images"]["maxItems"],
        MAX_VISION_IMAGES
    );
    assert_eq!(
        spec.schema["properties"]["question"]["maxLength"],
        MAX_VISION_QUESTION_CHARS
    );
}
