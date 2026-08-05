//! DeepSeek 形状的流式帧（`probes/PROVIDERS.md` §三），手法照抄
//! `agent-cli/tests/indep_support/sse.rs`——这里只需要最简单的纯文本回复。
#![allow(dead_code)]

/// 一段纯文本回复：一帧 `content` + `finish_reason: stop`，随后 `[DONE]`。
pub fn text_reply(text: &str) -> String {
    let content = serde_json::to_string(text).expect("json string");
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{content}}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n")
}
