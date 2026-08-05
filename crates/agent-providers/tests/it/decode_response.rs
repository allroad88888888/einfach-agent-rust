//! `decode(&Value) -> Decoded`：非流式响应体 → 中立结构。
//!
//! PROVIDERS.md §三：思考内容在 `delta.reasoning_content`，且排在 `content`
//! 之前流出——非流式响应体的字段名与嵌套结构跟流式帧一致，只是不分片
//! （骨架 OpenAI 兼容，`probes/results/wire-shape.json` 的流式帧与
//! `parallel.tool_calls` 记录共用同一套顶层字段：`id`/`object`/`created`/
//! `model`/`choices`/`usage`）。`parallel.tool_calls` 那条记录本身就是
//! `choices[0].message` 的内容，本文件把它套回完整的响应体形状
//! （加 `finish_reason` 和顶层 `usage`，usage 数值取自同一文件 deepseek 的
//! `stream.tail`）。

mod support;

use agent_core::{ContentBlock, StopReason};
use agent_providers::Provider;
use serde_json::json;

#[test]
fn reasoning_text_and_tool_calls_decode_in_order() {
    let provider = support::provider();

    // message 字段逐字取自 probes/results/wire-shape.json 的
    // deepseek.parallel.tool_calls；套进标准 chat.completion 响应壳。
    let body = json!({
        "id": "701eb37c-c0f1-4550-9c9e-02138ca0717e",
        "object": "chat.completion",
        "created": 1785483333,
        "model": "deepseek-v4-pro",
        "choices": [
            {
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": "好的，我来同时查询这两个信息。",
                    "reasoning_content": "用户要求同时查询北京的天气和上海的当前时间。我需要并行调用这两个工具。",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": "call_00_tsdA0CxopR9nvmG8nbrq3971",
                            "type": "function",
                            "function": { "name": "get_weather", "arguments": "{\"city\": \"北京\"}" }
                        },
                        {
                            "index": 1,
                            "id": "call_01_CJWrSIspIiAxEWS4xLq02947",
                            "type": "function",
                            "function": { "name": "get_time", "arguments": "{\"city\": \"上海\"}" }
                        }
                    ]
                }
            }
        ],
        "usage": {
            "prompt_tokens": 18,
            "completion_tokens": 15,
            "total_tokens": 33,
            "prompt_tokens_details": { "cached_tokens": 0 },
            "completion_tokens_details": { "reasoning_tokens": 13 },
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 18
        }
    });

    let decoded = provider.decode(&body);

    assert_eq!(
        decoded.blocks.len(),
        4,
        "应该是 Thinking + Text + 2 个 ToolUse，实际 blocks: {:?}",
        decoded.blocks
    );
    assert!(
        matches!(&decoded.blocks[0], ContentBlock::Thinking(t) if !t.is_empty()),
        "第 0 块必须是 Thinking，实际: {:?}",
        decoded.blocks[0]
    );
    assert!(
        matches!(&decoded.blocks[1], ContentBlock::Text(t) if !t.is_empty()),
        "第 1 块必须是 Text，实际: {:?}",
        decoded.blocks[1]
    );
    match &decoded.blocks[2] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id.0.as_ref(), "call_00_tsdA0CxopR9nvmG8nbrq3971");
            assert_eq!(name.as_ref(), "get_weather");
            assert_eq!(**input, json!({"city": "北京"}));
        }
        other => panic!("第 2 块必须是 ToolUse，实际: {other:?}"),
    }
    match &decoded.blocks[3] {
        ContentBlock::ToolUse { id, name, input } => {
            assert_eq!(id.0.as_ref(), "call_01_CJWrSIspIiAxEWS4xLq02947");
            assert_eq!(name.as_ref(), "get_time");
            assert_eq!(**input, json!({"city": "上海"}));
        }
        other => panic!("第 3 块必须是 ToolUse，实际: {other:?}"),
    }

    assert_eq!(decoded.stop, StopReason::ToolUse);
    assert_eq!(decoded.usage.prompt, 18);
    assert_eq!(decoded.usage.completion, 15);
    assert_eq!(decoded.usage.cached, Some(0));
}

/// 未知的 `finish_reason` 必须落到 `StopReason::Other`，**不许猜成 `EndTurn`**
/// ——猜错了 loop 会以为轮次正常结束。
#[test]
fn unknown_finish_reason_becomes_other_not_end_turn() {
    let provider = support::provider();

    let body = json!({
        "id": "x",
        "object": "chat.completion",
        "created": 1,
        "model": "deepseek-v4-pro",
        "choices": [
            {
                "index": 0,
                "finish_reason": "content_filter_or_something_未知",
                "message": {
                    "role": "assistant",
                    "content": "被截断了",
                    "reasoning_content": null
                }
            }
        ],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 3,
            "total_tokens": 13,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": 10
        }
    });

    let decoded = provider.decode(&body);
    assert!(
        matches!(decoded.stop, StopReason::Other(_)),
        "未知 finish_reason 必须是 StopReason::Other，实际: {:?}",
        decoded.stop
    );
    assert_ne!(decoded.stop, StopReason::EndTurn, "绝不能猜成 EndTurn");
}
