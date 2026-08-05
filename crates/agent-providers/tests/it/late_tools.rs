//! `late_tools` 非空时的行为（ADAPTER.md「中途加载工具的代价」+
//! probes/PROVIDERS.md §二）：DeepSeek 只能把晚加的工具并进顶层，本轮前缀作废，
//! 代价 120x——所以 `encode` 必须报 `Adjustment::LateToolsForcedIntoPrefix`，
//! 且这些工具必须真的出现在 `body` 里（不是「报了调整但其实没塞进去」）。

use crate::support;
use agent_core::{Adjustment, RequestIntent};
use agent_providers::Provider;

#[test]
fn non_empty_late_tools_are_reported_and_actually_present_in_body() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let messages = vec![support::user_text(1, "帮我查下天气和时间")];
    let tools = vec![support::tool_spec(
        "srv:get_weather",
        "query weather",
        serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )];
    let late_tools = vec![
        support::tool_spec(
            "srv:get_time",
            "query current time",
            serde_json::json!({"type": "object", "properties": {"tz": {"type": "string"}}}),
        ),
        support::tool_spec(
            "srv:get_stock_price",
            "query stock price",
            serde_json::json!({"type": "object", "properties": {"symbol": {"type": "string"}}}),
        ),
    ];
    let config = support::session_config();

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

    let reported_count = encoded.adjustments.iter().find_map(|a| match a {
        Adjustment::LateToolsForcedIntoPrefix { count, .. } => Some(*count),
        _ => None,
    });
    assert_eq!(
        reported_count,
        Some(2),
        "late_tools 有 2 个，adjustments 必须报 LateToolsForcedIntoPrefix{{count: 2, ..}}，\
         实际 adjustments: {:?}",
        encoded.adjustments
    );

    let body_text = String::from_utf8(encoded.body.clone()).expect("body 应该是合法 UTF-8 JSON");
    assert!(
        body_text.contains("get_time") || body_text.contains("srv:get_time"),
        "late_tools 里的工具必须真的进了 body（原名或映射名），body 里没找到 get_time"
    );
    assert!(
        body_text.contains("get_stock_price") || body_text.contains("srv:get_stock_price"),
        "late_tools 里的工具必须真的进了 body（原名或映射名），body 里没找到 get_stock_price"
    );
}
