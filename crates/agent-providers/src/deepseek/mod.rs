//! DeepSeek 适配（`providers.toml [default]` 指的第一家）。
//!
//! 这个文件只做**组装**：`encode` / `decode` / `classify` 各归各的子模块
//! （红线 9），这里把它们接到 [`Provider`] 上，外加这家的常量。序列化/前缀/
//! 工具表/错误分类的**机制**在 `crate::wire`（三家共用），这里只放**数据**——
//! 块粒度、工具上限、cached 路径、晚加工具的代价倍数——和这家独有的**判断**
//! （思考与强制工具调用互斥）。
//!
//! 这家的四条实测事实（PROVIDERS.md），决定了下面每处数据/判断的取值：
//! - 缓存**仅扩展**匹配、块 128、折扣 120x —— `CACHE_BLOCK`
//! - **没有消息级 tools**，晚加的只能并进顶层，代价约 120x —— `encode`
//! - 强制工具调用与思考模式互斥，必须同请求关思考 —— `encode`
//! - **402 是余额耗尽**，且未命中时 cached 字段**在但为 0** —— `CACHED_PATHS`

mod decode;
mod encode;
mod errors;

#[cfg(test)]
mod test_support;

use agent_core::ErrorClass;
use serde_json::Value;

use crate::wire;
use crate::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};

/// usage 里 cached token 的取值路径。这家未命中时**字段在、值为 0**
/// （`Some(0)`），跟字段整个缺失的 `None` 语义不同——只给这一条路径，
/// 就是不让 `prompt_tokens_details.cached_tokens` 之类的别家路径顺手兜底，
/// 那会把「这家没报」悄悄变成「报了 0」。
pub(crate) const CACHED_PATHS: &[&[&str]] = &[&["prompt_cache_hit_tokens"]];

/// 缓存块粒度：命中数总是 128 的整数倍向下取整（cache-prefix.json 实测
/// 434→384、574→512、966→896 三点全中）。
pub(crate) const CACHE_BLOCK: u32 = 128;

/// 工具数上限（providers.example.toml 记的官方值）。
pub(crate) const MAX_TOOLS: usize = 128;

/// 晚加工具并进顶层的估计代价倍数——仅扩展匹配下整条前缀作废，实测约 120x，
/// **别做**（PROVIDERS.md §二）。
pub(crate) const LATE_TOOLS_COST_MULTIPLE: f32 = 120.0;

/// 中途激活 skill 把正文拼进 system 段尾部的估计代价倍数（039）。
///
/// 038 实测：改现有 system 段尾部**保 ~91%** 前缀命中（对照插新 system 消息的
/// 120x 归零）。仅扩展匹配下，改动点之后（system 尾 + 整段 history）失配，约 9%
/// 的前缀落回全价、而 DeepSeek 缓存折扣最陡（未命中 vs 命中差 ~120x），量级估计
/// ≈ 0.91·1 + 0.09·120 ≈ 11。这是**激活那一跳**的上界；skill 稳定不变的后续跳
/// system 段逐字节相同、不漂、满命中，真实代价 ~1x（兜底第 2 层的 predicted vs
/// 实测对账会认出来）——这条 Adjustment 是「做了这个妥协」的**标记**，不是每跳账单。
pub(crate) const LATE_SYSTEM_COST_MULTIPLE: f32 = 11.0;

pub struct DeepSeek;

impl Provider for DeepSeek {
    fn encode(&self, ing: &Ingredients<'_>) -> Encoded {
        encode::encode(ing)
    }

    fn decode(&self, body: &Value) -> Decoded {
        decode::decode(body)
    }

    fn accumulator(&self) -> StreamAccumulator {
        // 工具名在 wire 上是转义过的（`wire::names`），流式路径也得还原回工具
        // 全名，否则 router 按名字找不到工具。
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
    use agent_core::{ContentBlock, StopReason, ToolCallId};
    use std::sync::Arc;
    use test_support::{ing, spec};

    /// 走 trait 的一整轮流式：分片工具调用拼完整、工具名还原、usage 拿得到。
    #[test]
    fn streaming_round_trip_through_trait() {
        let mut acc = DeepSeek.accumulator();
        for line in [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"reasoning_content":"要调工具"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"srv_3Afs_2Fread","arguments":"{\"pa"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\": \"/tmp"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"/a\"}"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":434,"completion_tokens":16,"prompt_cache_hit_tokens":384,"prompt_cache_miss_tokens":50}}"#,
            "data: [DONE]",
        ] {
            let events = acc.push_line(line);
            if let Some(StreamEvent::ToolCallStarted { name, .. }) = events.first() {
                assert_eq!(&**name, "srv:fs/read", "事件里的名字也要还原");
            }
        }
        assert!(acc.is_done());

        let (blocks, stop, usage) = acc.finish();
        assert_eq!(
            blocks,
            vec![
                ContentBlock::Thinking(Arc::from("要调工具")),
                ContentBlock::ToolUse {
                    id: ToolCallId::new("call_1"),
                    name: Arc::from("srv:fs/read"),
                    input: Arc::new(serde_json::json!({"path": "/tmp/a"})),
                },
            ]
        );
        assert_eq!(stop, StopReason::ToolUse);
        assert_eq!(usage.prompt, 434);
        assert_eq!(usage.cached, Some(384));
    }

    /// encode 出来的请求体是能直接发的形状：OpenAI 兼容 + 顶层 tools + 流式。
    #[test]
    fn encoded_body_is_wire_shaped() {
        let t = [spec("srv:fs/read")];
        let mut i = ing();
        i.tools = &t;
        let out = DeepSeek.encode(&i);
        let body: Value = serde_json::from_slice(&out.body).unwrap();
        assert_eq!(body["model"], serde_json::json!("deepseek-v4-pro"));
        assert_eq!(body["tools"][0]["type"], serde_json::json!("function"));
        assert_eq!(
            body["stream_options"]["include_usage"],
            serde_json::json!(true)
        );
        assert_eq!(out.prefix.segments.len(), 3);
    }
}
