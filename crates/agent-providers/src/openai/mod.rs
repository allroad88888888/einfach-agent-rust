//! **通用 OpenAI 兼容适配**（issue [175](../../../../docs/issues/175-openai-compat-decision.md)）。
//!
//! 跟另外三家的根本区别：它**不对应任何一家 provider**，对应的是一类端点
//! ——OpenAI 官方、Ollama、vLLM、OpenRouter、硅基流动，以及任何自称
//! 「OpenAI 兼容」的自研网关。因此这个文件里**没有一条实测事实**，
//! 另外三家的 `mod.rs` 顶上那种「块 64、门槛 860、上限 128」这里一条都写不出来
//! ——**对面是谁，事前不可知**。
//!
//! # 契约：只发最小内核
//!
//! `encode` 只发**每个兼容实现都必须支持**的字段：
//! `model` / `messages` / `max_tokens` / `stream` / `tools`（`tool_choice` 见下）。
//! `temperature`、`top_p`、`n`、`stream_options` 这些**一律不发**，取值交给对面的默认。
//!
//! 这不是保守，是把问题**消掉**。174 的实测两半：
//!
//! - 发全套 OpenAI 字段 → Kimi **400**：`temperature: 0.0` 被拒。0.0 是 OpenAI 的
//!   合法值，一个通用 adapter 没有任何理由知道这家只收 1.0。
//! - **只发最小内核** → 三家全过（含刚才 400 的 Kimi），带 `tools` 也全过。
//!
//! 于是「合法值被这家拒绝」这一整类在结构上不存在，**不需要任何 per-endpoint
//! 怪癖表**——那玩意就是 `match provider` 换个地方住（红线 12 的形状），
//! 还会把「配错了静默降级」的风险转嫁给不掌握细节的使用者。
//!
//! **代价照实说**：这个 adapter **给不了确定性采样**。要可复现输出，
//! 用专门那家的 adapter。它的定位是**够得着更多端点**，不是替代已适配的三家。
//!
//! # 三条「不知道就不猜」
//!
//! 1. **缓存参数**：块粒度与起效门槛对面是什么完全未知。取
//!    [`CACHE_BLOCK`] = 1、[`PREDICT_MIN`] = `u32::MAX`——**等于不预测**。
//!    宁可第 2 层兜底「无预测不判」，也不要拿一个瞎猜的数去跟真实 usage 对账，
//!    那会制造出一堆假告警，最后没人再看告警。
//! 2. **工具上限**：不知道，取 [`MAX_TOOLS`] = `usize::MAX`（不截断）。
//!    对面真有上限就会返回一个错误，那是**可见的失败**；我们自己先截掉
//!    才是静默的——模型会发现某个工具「不见了」，而没有任何地方说过它被丢了。
//! 3. **`base_url` 不许自己拼 `/v1`**：174 实测 GLM 的兼容端点是
//!    `/api/paas/v4/chat/completions`，**没有 `/v1`**。路径由用户在配置里带全，
//!    adapter 只负责在后面接 `/chat/completions`（跟三家的 `caps::endpoint` 同款语义）。
//!
//! # 缓存字段路径
//!
//! `prompt_tokens_details.cached_tokens`——OpenAI 官方口径。174 实测：DeepSeek
//! 同时给它和自己的 `prompt_cache_hit_tokens`，且**两个数一模一样**（1280/1280）；
//! GLM 只给这一条。**只填这一条，不做多路径兜底**——多路径兜底会让「这家到底
//! 报没报缓存」这件事变得说不清，而说不清正是 024 三层兜底最怕的输入。

mod decode;
mod encode;
mod errors;

#[cfg(test)]
#[path = "test_support.rs"]
mod test_support;

/// 198：缓存字段缺失不许被读成 0 的看门狗。单独一个文件是因为它守的是一条
/// **静默失效**（读成 0 会让 024 第 2 层永远「完美吻合」），值得让人一眼看见。
#[cfg(test)]
#[path = "usage_guard_tests.rs"]
mod usage_guard_tests;

use agent_core::ErrorClass;
use serde_json::Value;

use crate::wire;
use crate::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};

/// OpenAI 官方口径的缓存字段路径。理由见模块文档。
pub(crate) const CACHED_PATHS: &[&[&str]] = &[&["prompt_tokens_details", "cached_tokens"]];

/// 块粒度取 1 + [`PREDICT_MIN`] 取 `u32::MAX` ⇒ **恒不预测**。
/// 对面的缓存实现未知，瞎猜一个数去跟真实 usage 对账只会制造假告警。
pub(crate) const CACHE_BLOCK: u32 = 1;

/// 见 [`CACHE_BLOCK`]：`u32::MAX` 意味着任何前缀长度都低于门槛，`predicted_cache`
/// 恒为 0，第 2 层兜底按「无预测不判」处理。
pub(crate) const PREDICT_MIN: u32 = u32::MAX;

/// 不截断。对面真有上限就让它报错——那是可见的失败；我们自己先截才是静默的。
pub(crate) const MAX_TOOLS: usize = usize::MAX;

/// 通用 OpenAI 兼容端点。**无状态**——它不知道对面是谁，也不该知道。
pub struct OpenAiCompat;

impl Provider for OpenAiCompat {
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

/// 端点拼装：`<base_url>/chat/completions`。**`base_url` 由用户带全路径**，
/// 这里不补 `/v1`（174 结论一）。
pub fn endpoint(base_url: &str) -> String {
    format!("{}/chat/completions", base_url.trim_end_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 174 结论一：不许自己拼 `/v1`。用户填什么就是什么，只补 `/chat/completions`。
    #[test]
    fn endpoint_never_invents_a_v1_segment() {
        assert_eq!(
            endpoint("https://api.openai.com/v1"),
            "https://api.openai.com/v1/chat/completions"
        );
        // GLM 的兼容端点没有 /v1 —— 硬加就 404（174 的 glm_wrong_path_v1 观测）。
        assert_eq!(
            endpoint("https://open.bigmodel.cn/api/paas/v4"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        // 尾斜杠不该产生双斜杠。
        assert_eq!(
            endpoint("http://localhost:11434/v1/"),
            "http://localhost:11434/v1/chat/completions"
        );
    }

    /// 走 trait 的一整轮流式：共享累积器 + OpenAI 标准的 cached 路径。
    #[test]
    fn streaming_round_trip_reads_the_standard_cached_path() {
        use agent_core::ContentBlock;
        use std::sync::Arc;

        let mut acc = OpenAiCompat.accumulator();
        for line in [
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"he"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":"llo"}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1301,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":1280}}}"#,
            "data: [DONE]",
        ] {
            acc.push_line(line);
        }
        let (blocks, _, usage) = acc.finish();
        assert_eq!(blocks, vec![ContentBlock::Text(Arc::from("hello"))]);
        assert_eq!(usage.cached, Some(1280));
    }

    /// **不做多路径兜底**：只给 DeepSeek 自家那条路径时，读到的是「不知道」而不是
    /// 顺手兜底出来的数——说不清正是 024 三层兜底最怕的输入。
    #[test]
    fn a_vendor_only_cache_field_is_not_silently_picked_up() {
        let mut acc = OpenAiCompat.accumulator();
        acc.push_line(
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":434,"completion_tokens":16,"prompt_cache_hit_tokens":384}}"#,
        );
        acc.push_line("data: [DONE]");
        let (_, _, usage) = acc.finish();
        assert_eq!(usage.cached, None, "别家的私有路径不该被顺手兜底");
    }
}
