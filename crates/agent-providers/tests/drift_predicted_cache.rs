//! `drift` 与 `predicted_cache`（ADAPTER.md「缓存兜底也照这个切」+
//! probes/PROVIDERS.md §一）。DeepSeek 只认「严格延长」：新请求必须是已见过
//! 请求的严格延长，块粒度 128（PROVIDERS.md 速查表）。
//!
//! - 没有 `prev_prefix`：冷启动，`drift == None`、`predicted_cache == 0`。
//! - 用上一轮 `encode` 返回的 `prefix`（手动填一个 `prompt_tokens`）当
//!   `prev_prefix`，只在消息末尾追加一条：`drift == None`，
//!   `predicted_cache` 是 `prompt_tokens` 按 128 向下取整——分别测一次整除
//!   和不整除。
//! - 改工具表再 encode：`drift == Some(Segment::Tools)`，`predicted_cache == 0`
//!   （前缀归零，PROVIDERS.md「顶层 tools 在 prompt 最前面」）。
//! - 改中段历史（不是追加）：`drift == Some(Segment::History)`。

mod support;

use agent_core::{RequestIntent, Segment};
use agent_providers::Provider;

fn weather_tool() -> agent_core::ToolSpec {
    support::tool_spec(
        "srv:get_weather",
        "query weather",
        serde_json::json!({"type": "object", "properties": {"city": {"type": "string"}}}),
    )
}

fn time_tool() -> agent_core::ToolSpec {
    support::tool_spec(
        "srv:get_time",
        "query current time",
        serde_json::json!({"type": "object", "properties": {"tz": {"type": "string"}}}),
    )
}

#[test]
fn cold_start_without_prev_prefix_has_no_drift_and_zero_predicted_cache() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let messages = vec![support::user_text(1, "北京天气怎么样")];
    let tools = vec![weather_tool()];
    let late_tools: Vec<agent_core::ToolSpec> = vec![];
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
    assert_eq!(encoded.drift, None, "冷启动没有 prev_prefix，drift 必须是 None");
    assert_eq!(encoded.predicted_cache, 0, "冷启动没有可预测的命中，必须是 0");
}

/// 严格延长（只在消息末尾追加）：`prompt_tokens` 整除 128 的情形。
#[test]
fn strict_extension_predicted_cache_rounds_down_to_block_size_exact_multiple() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let tools = vec![weather_tool()];
    let late_tools: Vec<agent_core::ToolSpec> = vec![];
    let config = support::session_config();

    let messages_1 = vec![support::user_text(1, "北京天气怎么样")];
    let ing_1 = support::ingredients(
        &system,
        &messages_1,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );
    let first = provider.encode(&ing_1);

    // 宿主在拿到真实 usage 后回填 prompt_tokens，这里手动模拟：2432 = 19 * 128，
    // 整除块粒度。
    let mut prev_prefix = first.prefix.clone();
    prev_prefix.prompt_tokens = Some(2432);

    let messages_2 = vec![
        support::user_text(1, "北京天气怎么样"),
        support::assistant_text(2, "北京今天晴，25 度。"),
    ];
    let ing_2 = support::ingredients(
        &system,
        &messages_2,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        Some(&prev_prefix),
    );
    let second = provider.encode(&ing_2);

    assert_eq!(second.drift, None, "只在末尾追加消息，没有任何一段漂，drift 必须是 None");
    assert_eq!(
        second.predicted_cache, 2432,
        "2432 整除 128，向下取整后还是 2432"
    );
}

/// 严格延长：`prompt_tokens` 不整除 128 的情形（3100 / 128 = 24.2...，
/// 向下取整到 24 * 128 = 3072）。
#[test]
fn strict_extension_predicted_cache_rounds_down_to_block_size_non_multiple() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let tools = vec![weather_tool()];
    let late_tools: Vec<agent_core::ToolSpec> = vec![];
    let config = support::session_config();

    let messages_1 = vec![support::user_text(1, "北京天气怎么样")];
    let ing_1 = support::ingredients(
        &system,
        &messages_1,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );
    let first = provider.encode(&ing_1);

    let mut prev_prefix = first.prefix.clone();
    prev_prefix.prompt_tokens = Some(3100);

    let messages_2 = vec![
        support::user_text(1, "北京天气怎么样"),
        support::assistant_text(2, "北京今天晴，25 度。"),
    ];
    let ing_2 = support::ingredients(
        &system,
        &messages_2,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        Some(&prev_prefix),
    );
    let second = provider.encode(&ing_2);

    assert_eq!(second.drift, None);
    assert_eq!(
        second.predicted_cache, 3072,
        "3100 不整除 128，必须向下取整到 24 * 128 = 3072"
    );
}

/// 改工具表再 encode：顶层 tools 在 prompt 最前面（PROVIDERS.md §一），
/// 改了它整段前缀归零——`drift == Some(Segment::Tools)`，`predicted_cache == 0`。
#[test]
fn changing_tool_table_drifts_tools_segment_and_zeroes_predicted_cache() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let messages = vec![support::user_text(1, "北京天气怎么样")];
    let late_tools: Vec<agent_core::ToolSpec> = vec![];
    let config = support::session_config();

    let tools_1 = vec![weather_tool()];
    let ing_1 = support::ingredients(
        &system,
        &messages,
        &tools_1,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );
    let first = provider.encode(&ing_1);
    let mut prev_prefix = first.prefix.clone();
    prev_prefix.prompt_tokens = Some(2432);

    // 工具表变了：多了一个 time_tool。
    let tools_2 = vec![weather_tool(), time_tool()];
    let ing_2 = support::ingredients(
        &system,
        &messages,
        &tools_2,
        &late_tools,
        &config,
        RequestIntent::Free,
        Some(&prev_prefix),
    );
    let second = provider.encode(&ing_2);

    assert_eq!(second.drift, Some(Segment::Tools), "改了工具表，漂的必须是 Tools 段");
    assert_eq!(second.predicted_cache, 0, "前缀已经归零，没有可预测的命中");
}

/// 改中段历史（不是追加）：`drift == Some(Segment::History)`。
#[test]
fn rewriting_middle_history_drifts_history_segment() {
    let provider = support::provider();
    let system = vec![support::sys_chunk("base", "你是一个助手。")];
    let tools = vec![weather_tool()];
    let late_tools: Vec<agent_core::ToolSpec> = vec![];
    let config = support::session_config();

    let messages_1 = vec![
        support::user_text(1, "北京天气怎么样"),
        support::assistant_text(2, "北京今天晴，25 度。"),
        support::user_text(3, "那上海呢"),
    ];
    let ing_1 = support::ingredients(
        &system,
        &messages_1,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );
    let first = provider.encode(&ing_1);
    let mut prev_prefix = first.prefix.clone();
    prev_prefix.prompt_tokens = Some(2432);

    // 改写第 2 条消息（中段），不是在末尾追加。
    let messages_2 = vec![
        support::user_text(1, "北京天气怎么样"),
        support::assistant_text(2, "北京今天多云，18 度。"),
        support::user_text(3, "那上海呢"),
    ];
    let ing_2 = support::ingredients(
        &system,
        &messages_2,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        Some(&prev_prefix),
    );
    let second = provider.encode(&ing_2);

    assert_eq!(
        second.drift,
        Some(Segment::History),
        "改写的是中段历史而不是末尾追加，漂的必须是 History 段"
    );
}
