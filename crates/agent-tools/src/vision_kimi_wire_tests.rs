use super::*;
use serde_json::json;

#[test]
fn chat_body_matches_expected_shape_with_content_array_order_pinned() {
    let body = chat_body(
        "moonshot-v1-8k-vision-preview",
        "https://example.com/img.png",
        "what is this?",
    );

    assert_eq!(
        body,
        json!({
            "model": "moonshot-v1-8k-vision-preview",
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": "https://example.com/img.png" } },
                    { "type": "text", "text": "what is this?" }
                ]
            }]
        })
    );

    // Pin the array order explicitly too: content is a Vec (insertion order), not a
    // map key that serde_json would alphabetize. index 0 must be image_url, index 1 text.
    let content = body["messages"][0]["content"].as_array().unwrap();
    assert_eq!(content.len(), 2);
    assert_eq!(content[0]["type"], "image_url");
    assert_eq!(content[1]["type"], "text");
}

#[test]
fn chat_body_serialization_is_byte_for_byte_deterministic_across_1000_calls() {
    let first = serde_json::to_string(&chat_body("m", "file-ref-1", "question?")).unwrap();
    for _ in 0..1000 {
        let next = serde_json::to_string(&chat_body("m", "file-ref-1", "question?")).unwrap();
        assert_eq!(
            next, first,
            "chat_body serialization drifted across repeated calls with identical inputs"
        );
    }
}

#[test]
fn chat_body_escapes_special_characters_in_inputs_without_corrupting_payload() {
    let body = chat_body(
        "model \"x\"",
        "data:image/png;base64,AA==\n\"quote\"",
        "line1\nline2\t\"quoted\" 中文",
    );

    // Round-trip through a real JSON parser: if escaping were wrong this would either
    // fail to parse or silently corrupt one of the field values.
    let text = serde_json::to_string(&body).unwrap();
    let reparsed: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(reparsed, body);

    assert_eq!(reparsed["model"], "model \"x\"");
    assert_eq!(
        reparsed["messages"][0]["content"][0]["image_url"]["url"],
        "data:image/png;base64,AA==\n\"quote\""
    );
    assert_eq!(
        reparsed["messages"][0]["content"][1]["text"],
        "line1\nline2\t\"quoted\" 中文"
    );
}

#[test]
fn parse_content_happy_path_extracts_message_content_string() {
    let text = json!({
        "choices": [{
            "message": { "content": "a description of the image" }
        }]
    })
    .to_string();

    assert_eq!(
        parse_content(&text).unwrap(),
        "a description of the image"
    );
}

#[test]
fn parse_content_missing_choices_key_is_invalid_response() {
    let text = json!({ "not_choices": [] }).to_string();

    let err = parse_content(&text).unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn parse_content_choices_present_but_message_has_no_content_field_is_invalid_response() {
    let text = json!({
        "choices": [{ "message": {} }]
    })
    .to_string();

    let err = parse_content(&text).unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn parse_content_choices_present_but_content_is_not_a_string_is_invalid_response() {
    let text = json!({
        "choices": [{ "message": { "content": 123 } }]
    })
    .to_string();

    let err = parse_content(&text).unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn parse_content_empty_choices_array_is_invalid_response() {
    let text = json!({ "choices": [] }).to_string();

    let err = parse_content(&text).unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn parse_content_not_valid_json_is_invalid_response() {
    let err = parse_content("not json {").unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn parse_content_empty_string_is_invalid_response() {
    let err = parse_content("").unwrap_err();
    assert_eq!(err.code.as_ref(), "invalid_response");
}

#[test]
fn extension_for_known_image_mimes() {
    assert_eq!(extension_for("image/png"), "png");
    assert_eq!(extension_for("image/jpeg"), "jpg");
    assert_eq!(extension_for("image/webp"), "webp");
    assert_eq!(extension_for("image/gif"), "gif");
}

#[test]
fn extension_for_unknown_mime_falls_back_to_bin() {
    assert_eq!(extension_for("application/octet-stream"), "bin");
    assert_eq!(extension_for("image/bmp"), "bin");
    assert_eq!(extension_for(""), "bin");
}
