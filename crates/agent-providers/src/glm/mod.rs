//! GLM 适配（Zhipu GLM-5.2）。
//!
//! 序列化机制在 `crate::wire`（三家共用），这个文件只放这家的数据（块粒度、
//! 工具上限、usage 的 cached 路径、晚加工具的代价倍数）。四条实测事实
//! （PROVIDERS.md）决定了下面每处数据/判断的取值：
//!
//! - 缓存是三家里唯一的**真前缀树**匹配，块 64、起效门槛 ~860、命中折扣 2x。
//!   我们的 drift/predicted 计算沿用跟另两家一样的「仅扩展」规则（`wire::prefix`，
//!   块 64）——真前缀树只会比这条规则命中得更好、不会更差，所以这是一个
//!   **保守低估**，好于预期不用告警，不需要为 GLM 单独建一套真前缀树模型
//!   （那需要服务端才有的信息，见 `wire::prefix` 的模块文档）。
//! - **没有消息级 tools**：中途加载的工具跟 DeepSeek 一样只能并进顶层，本轮
//!   前缀作废，代价约 2 倍（比 DeepSeek 的 120 倍轻得多——真前缀树对「顶层变了
//!   全前缀作废」的惩罚小）——`encode` 报 `Adjustment::LateToolsForcedIntoPrefix`。
//! - **`tool_choice` 四种取值全支持**（PROVIDERS.md 明确指出「文档说只支持
//!   `auto` 是错的，以实测为准」）：`MustUse(name)` 直接翻译成指定函数，无
//!   调整——除非是我们自己截断掉的，那种情况跟 DeepSeek 一样报
//!   `ToolChoiceDowngraded`。
//! - **思考可开关，默认关**，且 `thinking.type` 一旦发送就会进缓存前缀（改
//!   一下前缀就全部作废）。**M1 不发 `thinking` 字段**（默认关，省了这个
//!   决定）——真要支持切换，这个字段要并进 `SegmentBytes.system`，不能事后
//!   再加，否则前一轮的前缀镜像就对不上了。
//! - temperature 自由，原样透传，不产生调整。
//! - usage 的 cached 路径跟 Kimi 一样是 `prompt_tokens_details.cached_tokens`，
//!   但**未命中时这家显式给 0**——`CACHED_PATHS` 复用同一条路径，`Some(0)`
//!   还是 `None` 完全由响应体本身决定，不需要额外代码区分。

mod decode;
mod encode;
mod errors;

#[cfg(test)]
mod test_support;

use agent_core::ErrorClass;
use serde_json::Value;

use crate::wire;
use crate::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};

pub(crate) const CACHED_PATHS: &[&[&str]] = &[&["prompt_tokens_details", "cached_tokens"]];

/// 缓存块粒度：真前缀树语义下这是我们能保证的下界，不是真实匹配算法
/// （`wire::prefix` 的模块文档）。
pub(crate) const CACHE_BLOCK: u32 = 64;

/// 起效门槛的夹逼上界（实测 ~460 完全不缓存、~860 跳到 98%，PROVIDERS.md §一）。
/// 之下**不预测**（predicted=0，第 2 层视为无预测不判）——真实两轮实测过
/// 526 token 的第 2 轮 predicted=448 / actual=0 的误报，零区里按块取整是确定地错。
pub(crate) const PREDICT_MIN: u32 = 860;

/// 工具数上限（PROVIDERS.md：跟 DeepSeek 一样是 128）。
pub(crate) const MAX_TOOLS: usize = 128;

/// 晚加工具并进顶层的估计代价倍数：真前缀树对「顶层变了」的惩罚比仅扩展匹配
/// 轻得多，实测约 2x（PROVIDERS.md §二）。
pub(crate) const LATE_TOOLS_COST_MULTIPLE: f32 = 2.0;

pub struct Glm;

impl Provider for Glm {
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
    use agent_core::ContentBlock;
    use std::sync::Arc;
    use test_support::{ing, spec};

    /// 走 trait 的一整轮流式：GLM 每帧重复 `role: "assistant"`，共享累积器已经
    /// 处理，不该污染文本（PROVIDERS.md §三）。
    #[test]
    fn streaming_round_trip_ignores_repeated_role() {
        let mut acc = Glm.accumulator();
        for line in [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"你"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"好"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":0}}}"#,
            "data: [DONE]",
        ] {
            acc.push_line(line);
        }
        let (blocks, _, usage) = acc.finish();
        assert_eq!(blocks, vec![ContentBlock::Text(Arc::from("你好"))]);
        // 未命中时这家给显式 0，不是 None。
        assert_eq!(usage.cached, Some(0));
    }

    /// encode 出来的请求体是能直接发的形状；M1 不带 `thinking` 字段。
    #[test]
    fn encoded_body_has_no_thinking_field_in_m1() {
        let t = [spec("srv:fs/read")];
        let mut i = ing();
        i.tools = &t;
        let out = Glm.encode(&i);
        let body: serde_json::Value = serde_json::from_slice(&out.body).unwrap();
        assert!(body.get("thinking").is_none());
        assert_eq!(body["tools"][0]["type"], serde_json::json!("function"));
    }
}
