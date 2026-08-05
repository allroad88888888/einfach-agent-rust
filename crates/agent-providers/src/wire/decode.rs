//! 非流式响应体 → 中立的 [`Decoded`]，三家共用。骨架一致（PROVIDERS.md：
//! 流式/错误骨架都明确是「OpenAI 兼容」，非流式响应体同样是
//! `choices[0].message` 形状，没有测出任何一家在这一层有独立差异），所以这是
//! 一份共享实现，各家只把自己 usage 的 cached 取值路径传进来。
//!
//! 块的顺序跟 wire 的流出顺序一致：`reasoning_content` 在 `content` 之前
//! （三家实测都是这样，PROVIDERS.md §三），所以 `Thinking` 排在 `Text` 前。
//!
//! 工具名要走 [`names::from_wire`] 还原成工具全名——`encode` 转义过，这里不
//! 还原的话 router 按名字找不到工具，而且是「模型好像没调这个工具」这种难查
//! 的症状。

use std::sync::Arc;

use agent_core::{ContentBlock, TokenUsage};
use serde_json::Value;

use super::names;
use crate::Decoded;
use crate::stream::{stop, tool_parts::ToolParts, usage};

pub fn decode(body: &Value, cached_paths: &[&[&str]]) -> Decoded {
    let choice = body
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|c| c.first());
    let message = choice.and_then(|c| c.get("message"));

    Decoded {
        blocks: message.map(blocks).unwrap_or_default(),
        // `finish_reason` 认不出或缺失一律 `Other`，**绝不猜成 `EndTurn`**。
        stop: choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(Value::as_str)
            .map_or_else(stop::missing, stop::from_wire),
        usage: usage::find(body).map_or(
            // 响应里没有 usage：`cached: None` = 这轮没人报，跟 `Some(0)`
            // （报了、确实没命中）不是一回事。
            TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None,
            },
            |u| usage::parse(u, cached_paths),
        ),
    }
}

fn blocks(message: &Value) -> Vec<ContentBlock> {
    let mut out = Vec::new();
    if let Some(t) = text_of(message, "reasoning_content") {
        out.push(ContentBlock::Thinking(Arc::from(t)));
    }
    if let Some(t) = text_of(message, "content") {
        out.push(ContentBlock::Text(Arc::from(t)));
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        let mut parts = ToolParts::default();
        for call in calls {
            parts.absorb(call);
        }
        out.extend(parts.into_blocks(names::from_wire));
    }
    out
}

/// `null`、字段省略、空串，三种都算「没有」——不能用「字段存在」判断有没有内容。
fn text_of<'a>(message: &'a Value, key: &str) -> Option<&'a str> {
    message
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{StopReason, ToolCallId};
    use serde_json::json;

    const DEEPSEEK: &[&[&str]] = &[&["prompt_cache_hit_tokens"]];

    /// 录制的并行工具调用响应（wire-shape.json `parallel.tool_calls`）。
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
        let d = decode(&body, DEEPSEEK);
        assert_eq!(d.stop, StopReason::ToolUse);
        assert_eq!(
            d.usage,
            TokenUsage {
                prompt: 100,
                completion: 20,
                cached: Some(64)
            }
        );
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
        match &decode(&body, DEEPSEEK).blocks[0] {
            ContentBlock::ToolUse { name, .. } => assert_eq!(&**name, "srv:fs/read"),
            other => panic!("期望 ToolUse，拿到 {other:?}"),
        }
    }

    /// `"content": null` 不产出空块；未知 / 缺失的 finish_reason 落 `Other`；
    /// usage 整个缺失时 `cached` 是 `None`。
    #[test]
    fn null_content_unknown_stop_and_missing_usage() {
        let d = decode(
            &json!({"choices": [{
                "finish_reason": "insufficient_system_resource",
                "message": {"role": "assistant", "content": null, "reasoning_content": ""}
            }]}),
            DEEPSEEK,
        );
        assert!(d.blocks.is_empty(), "{:?}", d.blocks);
        assert_eq!(
            d.stop,
            StopReason::Other(Arc::from("insufficient_system_resource"))
        );
        assert_eq!(
            d.usage,
            TokenUsage {
                prompt: 0,
                completion: 0,
                cached: None
            }
        );

        let d = decode(
            &json!({"choices": [{"message": {"content": "x"}}]}),
            DEEPSEEK,
        );
        assert_eq!(d.stop, StopReason::Other(Arc::from("missing")));

        // 空响应体也不许 panic，也不许猜成正常结束。
        let d = decode(&json!({}), DEEPSEEK);
        assert!(d.blocks.is_empty());
        assert_eq!(d.stop, StopReason::Other(Arc::from("missing")));
    }

    /// 未命中时字段整个缺失（Kimi 的路径）→ `cached: None`，不是 `Some(0)`。
    #[test]
    fn cached_path_missing_entirely_is_none() {
        const KIMI: &[&[&str]] = &[&["prompt_tokens_details", "cached_tokens"]];
        let body = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": "好"}}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 3}
        });
        assert_eq!(decode(&body, KIMI).usage.cached, None);
    }
}
