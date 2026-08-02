//! DeepSeek 的非流式响应体 → 中立的 [`Decoded`]。解析机制在 `crate::wire::decode`
//! （三家共用，PROVIDERS.md：响应体形状没有测出独立差异），这里只传这家的
//! cached 路径。

use serde_json::Value;

use super::CACHED_PATHS;
use crate::Decoded;
use crate::wire;

pub(crate) fn decode(body: &Value) -> Decoded {
    wire::decode::decode(body, CACHED_PATHS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{ContentBlock, StopReason, TokenUsage, ToolCallId};
    use serde_json::json;
    use std::sync::Arc;

    /// 录制的并行工具调用响应（wire-shape.json `parallel.tool_calls`），走这家
    /// 自己的 `decode` 入口（而不是直接调 `wire::decode`），确认接线正确。
    #[test]
    fn recorded_parallel_tool_calls() {
        let body = json!({
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant",
                "content": "好的，我来同时查询这两个信息。",
                "reasoning_content": "用户要求同时查询北京的天气和上海的当前时间。",
                "tool_calls": [
                    {"index": 0, "id": "call_00_tsdA", "type": "function",
                     "function": {"name": "get_weather", "arguments": "{\"city\": \"北京\"}"}},
                    {"index": 1, "id": "call_01_CJWr", "type": "function",
                     "function": {"name": "get_time", "arguments": "{\"city\": \"上海\"}"}}
                ]
            }}],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20,
                      "prompt_cache_hit_tokens": 64, "prompt_cache_miss_tokens": 36}
        });
        let d = decode(&body);
        assert_eq!(d.stop, StopReason::ToolUse);
        assert_eq!(d.usage, TokenUsage { prompt: 100, completion: 20, cached: Some(64) });
        assert_eq!(d.blocks.len(), 4, "思考 + 文本 + 两次调用：{:?}", d.blocks);
        assert!(matches!(d.blocks[0], ContentBlock::Thinking(_)));
        assert!(matches!(d.blocks[1], ContentBlock::Text(_)));
        assert_eq!(
            d.blocks[2],
            ContentBlock::ToolUse {
                id: ToolCallId::new("call_00_tsdA"),
                name: Arc::from("get_weather"),
                input: Arc::new(json!({"city": "北京"})),
            }
        );
    }

    /// 转义过的工具名要还原回工具全名。
    #[test]
    fn tool_name_is_unescaped() {
        let body = json!({"choices": [{"finish_reason": "tool_calls", "message": {
            "tool_calls": [{"index": 0, "id": "c1", "type": "function",
                "function": {"name": "srv_3Afs_2Fread", "arguments": "{}"}}]
        }}]});
        match &decode(&body).blocks[0] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(&**name, "srv:fs/read"),
            other => panic!("期望 ToolUse，拿到 {other:?}"),
        }
    }

    /// DeepSeek 未命中时 cached 字段**在但为 0**，跟 Kimi 的「整个缺失」不同。
    #[test]
    fn miss_is_some_zero_not_none() {
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "好"}}],
            "usage": {"prompt_tokens": 18, "completion_tokens": 15,
                      "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 18}
        });
        assert_eq!(decode(&body).usage.cached, Some(0));
    }
}
