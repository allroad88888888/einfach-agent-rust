//! 一条**静默失效**的看门狗（issue 198）：缓存字段缺失时 `cached` 必须是
//! `None`（「不知道」），不许是 `Some(0)`（「确定没命中」）。
//!
//! # 为什么这条值钱
//!
//! 读成 0 的后果全程不报错：
//!
//! 1. [`crate::openai`] 的 `predicted_cache` **恒为 0**（对面的缓存参数未知，
//!    决策见该模块文档「三条不知道就不猜」）
//! 2. 024 的第 2 层兜底拿 `predicted_cache` 跟真实 `usage.cached` 对账
//! 3. 若 `cached` 也被读成 `Some(0)`，每一轮都是「预测 0、实际 0、完美吻合」
//! 4. **那道闸从此永远不响——而它看起来一直在正常工作**
//!
//! 这正是 CLAUDE.md §红线摘要里那一类：「功能完全正常，只是每一轮都全价」。
//! `None` 会被第 2 层当成「无实测可对账」跳过，不产生假的吻合。
//!
//! # 为什么是单测而不是真机
//!
//! 174 实测三家（DeepSeek/Kimi/GLM）**都**返回了缓存字段，真机上碰不到这个分支。
//! 而一个不返回缓存字段的假响应就能钉死它，**并且能进 CI 永久守着**——
//! 一次性真机验证证明的是「那天没坏」，单测证明的是「以后坏了会红」。

use agent_core::TokenUsage;
use serde_json::json;

use crate::Provider;
use crate::openai::{OpenAiCompat, decode};

/// 非流式：`usage` 里**完全没有**任何缓存字段 → `None`。
///
/// 这是本组的主断言。一个自研网关或精简实现不给这个字段是完全正常的
/// （OpenAI 规范里 `prompt_tokens_details` 本来就是可选的）。
#[test]
fn a_response_without_any_cache_field_reports_unknown_not_zero() {
    let body = json!({
        "choices": [{"finish_reason": "stop", "message": {"content": "hi"}}],
        "usage": {"prompt_tokens": 128, "completion_tokens": 4}
    });
    let usage = decode::decode(&body).usage;
    assert_eq!(
        usage,
        TokenUsage {
            prompt: 128,
            completion: 4,
            cached: None,
        },
        "缺失必须是 None（不知道）——读成 Some(0) 会让 024 第 2 层每轮都「完美吻合」，永远不响"
    );
}

/// 非流式：**显式** `cached_tokens: 0` → `Some(0)`。
///
/// **跟上一条是两件事，必须都在。** 只测上一条的话，一个把两者都返回 `None`
/// 的实现也能过——那会丢掉「这家确实没命中」这个真实信号，
/// 等于把假绿换成了假红。
#[test]
fn an_explicit_zero_is_a_real_measurement_not_unknown() {
    let body = json!({
        "choices": [{"finish_reason": "stop", "message": {"content": "hi"}}],
        "usage": {
            "prompt_tokens": 128,
            "completion_tokens": 4,
            "prompt_tokens_details": {"cached_tokens": 0}
        }
    });
    assert_eq!(decode::decode(&body).usage.cached, Some(0));
}

/// 流式：末帧的 `usage` 没有缓存字段 → `None`。
///
/// 流式与非流式走的是两条解析路径（`stream::usage` vs `wire::decode`），
/// 两边都要守——只守一边的话，生产路径（流式是默认）恰好是没守住的那边。
#[test]
fn a_stream_without_any_cache_field_reports_unknown_not_zero() {
    let mut acc = OpenAiCompat.accumulator();
    acc.push_line(r#"data: {"choices":[{"index":0,"delta":{"content":"hi"}}]}"#);
    acc.push_line(
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":128,"completion_tokens":4}}"#,
    );
    acc.push_line("data: [DONE]");
    let (_, _, usage) = acc.finish();
    assert_eq!(usage.cached, None);
    assert_eq!(usage.prompt, 128, "prompt/completion 仍然要读到");
}

/// 流式：只给**别家的私有路径**（DeepSeek 的 `prompt_cache_hit_tokens`）→ `None`。
///
/// 通用 adapter 只认 OpenAI 标准那一条路径，**不做多路径兜底**
/// （[`crate::openai`] 模块文档）。多路径兜底会让「这家到底报没报缓存」
/// 变得说不清，而说不清正是 024 三层兜底最怕的输入。
#[test]
fn another_vendors_private_cache_path_is_not_silently_adopted() {
    let mut acc = OpenAiCompat.accumulator();
    acc.push_line(
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":434,"completion_tokens":16,"prompt_cache_hit_tokens":384}}"#,
    );
    acc.push_line("data: [DONE]");
    let (_, _, usage) = acc.finish();
    assert_eq!(usage.cached, None, "别家的私有路径不该被顺手兜底");
}
