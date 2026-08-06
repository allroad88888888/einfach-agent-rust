use serde_json::{Value, json};

use super::fixture::{QUESTION, VISION_WIRE_NAME};
use crate::support;

pub fn selects_second_image(body: &str) -> String {
    select_image(body, 1)
}

pub fn selects_first_image(body: &str) -> String {
    select_image(body, 0)
}

pub fn handles_from_root_request(body: &str) -> Vec<String> {
    let body: Value = serde_json::from_str(body).expect("DeepSeek request JSON");
    let text = body["messages"]
        .as_array()
        .expect("DeepSeek messages")
        .iter()
        .filter_map(|message| message["content"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    scan_handles(&text)
}

fn select_image(body: &str, index: usize) -> String {
    let handles = handles_from_root_request(body);
    let Some(selected) = handles.get(index) else {
        return support::wire::text_reply("ROOT_DID_NOT_RECEIVE_EXPECTED_HANDLES");
    };
    let arguments = json!({"images": [selected], "question": QUESTION}).to_string();
    let tool = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "index": 0,
                    "id": "call_vision",
                    "type": "function",
                    "function": {"name": VISION_WIRE_NAME, "arguments": arguments}
                }]
            },
            "finish_reason": Value::Null
        }]
    });
    let finish = json!({
        "choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]
    });
    format!("data: {tool}\n\ndata: {finish}\n\ndata: [DONE]\n\n")
}

fn scan_handles(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut remaining = text;
    while let Some(offset) = remaining.find("img_") {
        let tail = &remaining[offset..];
        let digits = tail[4..]
            .chars()
            .take_while(|character| character.is_ascii_digit())
            .count();
        if digits > 0 {
            let handle = tail[..4 + digits].to_string();
            if !found.contains(&handle) {
                found.push(handle);
            }
        }
        remaining = &tail[4 + digits..];
    }
    found
}
