//! `intent: RequestIntent` 在 DeepSeek 上的翻译路径（ADAPTER.md 的 `intent` 表 +
//! probes/PROVIDERS.md §二「强制工具调用与思考模式互斥」）。
//!
//! 实测依据：DeepSeek v4-pro 默认开着思考；`required` / 指定函数在默认思考下
//! 直接 400（错误原文 "Thinking mode does not support this tool_choice"），
//! 显式关思考后才都能用。所以 adapter 遇到 `MustUseTool` / `MustUse(..)` 必须
//! 自动关思考，并记一笔 `Adjustment::ThinkingDisabledForToolChoice`——静默改变
//! 模型行为是本层头号大忌。

mod support;

use agent_core::{Adjustment, RequestIntent};
use agent_providers::Provider;

fn base_ingredients_parts() -> (Vec<agent_core::SystemChunk>, Vec<agent_core::Message>, Vec<agent_core::ToolSpec>, agent_core::SessionConfig) {
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let messages = vec![support::user_text(1, "北京天气怎么样")];
    let tools = vec![support::tool_spec(
        "srv:get_weather",
        "query weather",
        serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )];
    let config = support::session_config();
    (system, messages, tools, config)
}

#[test]
fn free_intent_produces_no_adjustments() {
    let provider = support::provider();
    let (system, messages, tools, config) = base_ingredients_parts();
    let late_tools: Vec<agent_core::ToolSpec> = vec![];

    let ing = support::ingredients(
        &system,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );

    let encoded = provider.encode(&ing);
    assert!(
        encoded.adjustments.is_empty(),
        "Free 意图在没有其他触发条件时不该产生任何 Adjustment，实际: {:?}",
        encoded.adjustments
    );
}

#[test]
fn must_use_named_tool_disables_thinking_on_deepseek() {
    let provider = support::provider();
    let (system, messages, tools, config) = base_ingredients_parts();
    let late_tools: Vec<agent_core::ToolSpec> = vec![];

    let ing = support::ingredients(
        &system,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::MustUse(std::sync::Arc::from("srv:get_weather")),
        None,
    );

    let encoded = provider.encode(&ing);
    assert!(
        encoded.adjustments.contains(&Adjustment::ThinkingDisabledForToolChoice),
        "DeepSeek 默认开思考，MustUse 必须先关思考才能传，adjustments: {:?}",
        encoded.adjustments
    );
}

#[test]
fn must_use_tool_disables_thinking_on_deepseek() {
    let provider = support::provider();
    let (system, messages, tools, config) = base_ingredients_parts();
    let late_tools: Vec<agent_core::ToolSpec> = vec![];

    let ing = support::ingredients(
        &system,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::MustUseTool,
        None,
    );

    let encoded = provider.encode(&ing);
    assert!(
        encoded.adjustments.contains(&Adjustment::ThinkingDisabledForToolChoice),
        "MustUseTool 等价于 tool_choice=required，DeepSeek 默认思考下这条也是 400，\
         必须先关思考，adjustments: {:?}",
        encoded.adjustments
    );
}
