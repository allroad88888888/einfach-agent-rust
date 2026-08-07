//! Kimi 适配（Moonshot K3）。
//!
//! 序列化机制在 `crate::wire`（三家共用），这个文件只放这家的数据（块粒度、
//! usage 的 cached 路径）和这家独有的判断（消息级 tools、思考常开、
//! temperature 锁 1）。五条实测事实（PROVIDERS.md）决定了下面的形状：
//!
//! - 缓存**仅扩展**匹配、块 256、起效门槛 256、命中折扣 10x —— `CACHE_BLOCK`
//! - **有消息级 tools**：中途加载的工具追加成 `role:system` + `tools`（无
//!   `content`）的消息，零缓存代价——`encode` 里单独处理，不走
//!   `Adjustment::LateToolsForcedIntoPrefix`（PROVIDERS.md §二「中途加载」）
//! - **思考常开、关不掉**：指定函数（`tool_choice` 指定名字）永久 400
//!   （错误原文 `tool_choice 'specified' is incompatible with thinking
//!   enabled`），`encode` 无条件降级成 `required` 并报
//!   `Adjustment::ToolChoiceDowngraded`；`MustUseTool` 直接可用，无调整
//! - **temperature 只接受 1**：非 1 的值在 `encode` 里被钳成 1.0，报
//!   `Adjustment::TemperatureOverridden`；`None` 不传，不调整
//! - usage 的 cached token 走 `prompt_tokens_details.cached_tokens`，**未命中
//!   时这个路径整个缺失**（不是 0）——只给这一条 `CACHED_PATHS`，
//!   `wire::decode` / `StreamAccumulator` 的路径解析天然给出 `None` 语义，
//!   不需要额外代码
//!
//! 404 `resource_not_found_error`（模型名错）/ 429 `engine_overloaded_error` /
//! 401 都由共享的 `wire::errors::classify` 按 `error.type` 关键词判出，不需要
//! Kimi 专属规则（`errors.rs` 有这三条的回归测试）。
//!
//! 工具数上限「未公布」（PROVIDERS.md）：不编一个没测过的数字，顶层 `tools`
//! 和消息级 `late_tools` 都不做截断，`Adjustment::ToolsTruncated` 在这家因此
//! 永远不会出现。

mod decode;
mod encode;
#[cfg(test)]
mod encode_tests;
mod errors;
mod late_tools;

#[cfg(test)]
mod test_support;

use agent_core::ErrorClass;
use serde_json::Value;

use crate::wire;
use crate::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};

/// usage 里 cached token 的取值路径。未命中时**字段整个缺失**（`None`），
/// 跟 DeepSeek/GLM 未命中给显式 `0`（`Some(0)`）不同。
pub(crate) const CACHED_PATHS: &[&[&str]] = &[&["prompt_tokens_details", "cached_tokens"]];

/// 缓存块粒度：仅扩展匹配，起效门槛也是 256（PROVIDERS.md §一：470 tokens 时
/// 只缓存 1 块，浪费 214）。
pub(crate) const CACHE_BLOCK: u32 = 256;

pub struct Kimi;

impl Provider for Kimi {
    fn encode(&self, ing: &Ingredients<'_>) -> Encoded {
        encode::encode(ing)
    }

    fn decode(&self, body: &Value) -> Decoded {
        decode::decode(body)
    }

    fn accumulator(&self) -> StreamAccumulator {
        StreamAccumulator::new(CACHED_PATHS).with_name_from_wire(wire::names::from_wire)
    }

    fn classify(&self, status: u16, body: &str) -> ErrorClass {
        errors::classify(status, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::StreamEvent;
    use agent_core::{Adjustment, ContentBlock, RequestIntent, StopReason};
    use std::sync::Arc;
    use test_support::{ing, spec};

    /// 走 trait 的一整轮流式：跟另两家共用同一个 `StreamAccumulator`，尾帧
    /// `choices` 为空也拿得到 usage（PROVIDERS.md §三「Kimi 的 usage 在 finish
    /// 帧之后另起一帧」）。
    #[test]
    fn streaming_round_trip_through_trait_with_trailing_usage_frame() {
        let mut acc = Kimi.accumulator();
        let mut saw_usage = false;
        for line in [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"好"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
            r#"data: {"choices":[],"usage":{"prompt_tokens":110,"completion_tokens":5,"prompt_tokens_details":{"cached_tokens":110}}}"#,
            "data: [DONE]",
        ] {
            for ev in acc.push_line(line) {
                if matches!(ev, StreamEvent::UsageReady(_)) {
                    saw_usage = true;
                }
            }
        }
        assert!(saw_usage, "尾帧 choices 为空也不能丢 usage");
        let (blocks, stop, usage) = acc.finish();
        assert_eq!(blocks, vec![ContentBlock::Text(Arc::from("好"))]);
        assert_eq!(stop, StopReason::EndTurn);
        assert_eq!(usage.cached, Some(110));
    }

    /// `MustUse(name)` 在 Kimi 上永久做不到，`encode` 必须报降级而不是静默翻译。
    #[test]
    fn kimi_must_use_named_tool_always_downgrades() {
        let t = [spec("srv:fs/read")];
        let mut i = ing();
        i.tools = &t;
        i.intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
        let out = Kimi.encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["tool_choice"], serde_json::json!("required"));
        assert_eq!(
            out.adjustments,
            vec![Adjustment::ToolChoiceDowngraded {
                wanted: Arc::from("srv:fs/read"),
                used: Arc::from("required"),
            }]
        );
    }
}
