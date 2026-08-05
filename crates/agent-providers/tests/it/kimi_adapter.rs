//! Kimi adapter 的黑盒验收测试（issue 023）。
//!
//! 独立测试 agent 规则：只依据 `docs/issues/023-three-providers.md` 的验收条目、
//! `probes/PROVIDERS.md` 的实测数据、`docs/INVARIANTS.md` 红线 11/12、
//! `docs/ADAPTER.md` 的接缝定义，以及 `agent-providers` / `agent-core` 的公开签名。
//! **不看** `crates/agent-providers/src/kimi/` 的实现体。

mod support;

use std::sync::Arc;

use agent_core::{Adjustment, ErrorClass, RequestIntent, SessionConfig, ToolSpec};
use agent_providers::kimi::Kimi;
use agent_providers::{Provider, StreamEvent};

use support::{ingredients, sys_chunk, tool_spec, user_text};

fn kimi_config(temperature: Option<f32>) -> SessionConfig {
    SessionConfig {
        model: Arc::from("kimi-k3"),
        temperature,
        max_tokens: Some(4096),
        context_window: Some(128_000),
    }
}

// ---------------------------------------------------------------------
// 1. MustUse(name) 在 Kimi 上永久不可用（PROVIDERS.md §二），必须降级并记一笔；
//    MustUseTool（不指定哪个）原生支持，不该有调整。
// ---------------------------------------------------------------------

/// §二状态表：Kimi 上「指定函数」永久不可用（思考常开、API 里没有关闭字段）。
/// adapter 必须降级成 `required` 并如实记一笔 `ToolChoiceDowngraded`——静默降级
/// 是接缝的头号大忌（ADAPTER.md 自查表第四条）。
#[test]
fn must_use_named_tool_downgrades_with_adjustment() {
    let sys = [sys_chunk("base", "you are a helpful agent")];
    let messages = [user_text(1, "read the file")];
    let tools = [tool_spec("srv:fs/read", "read a file", serde_json::json!({"type": "object"}))];
    let late_tools: [ToolSpec; 0] = [];
    let config = kimi_config(None);

    let ing = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::MustUse(Arc::from("srv:fs/read")),
        None,
    );

    let out = Kimi.encode(&ing);
    assert_eq!(
        out.adjustments,
        vec![Adjustment::ToolChoiceDowngraded {
            wanted: Arc::from("srv:fs/read"),
            used: Arc::from("required"),
        }],
        "Kimi 上指定函数永久不可用，必须降级成 required 并记一笔：{:?}",
        out.adjustments
    );
}

/// 同一份料换成 `MustUseTool`（不要求指定哪个）：Kimi 原生支持 `required`，
/// 不需要降级——adjustments 必须是空的，原样执行了就不该多报一条。
#[test]
fn must_use_tool_no_name_needs_no_adjustment() {
    let sys = [sys_chunk("base", "you are a helpful agent")];
    let messages = [user_text(1, "read something")];
    let tools = [tool_spec("srv:fs/read", "read a file", serde_json::json!({"type": "object"}))];
    let late_tools: [ToolSpec; 0] = [];
    let config = kimi_config(None);

    let ing = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::MustUseTool,
        None,
    );

    let out = Kimi.encode(&ing);
    assert!(
        out.adjustments.is_empty(),
        "MustUseTool 在 Kimi 上原生支持，不该有调整：{:?}",
        out.adjustments
    );
}

// ---------------------------------------------------------------------
// 2. temperature：速查表「只接受 1」——传别的要被钉死并记一笔；不传不该凭空报。
// ---------------------------------------------------------------------

#[test]
fn temperature_override_when_set() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "hi")];
    let tools: [ToolSpec; 0] = [];
    let late_tools: [ToolSpec; 0] = [];
    let config = kimi_config(Some(0.7));

    let ing = ingredients(&sys, &messages, &tools, &late_tools, &config, RequestIntent::Free, None);

    let out = Kimi.encode(&ing);
    assert!(
        out.adjustments.contains(&Adjustment::TemperatureOverridden { wanted: 0.7, used: 1.0 }),
        "temperature=Some(0.7) 必须被钉死成 1 并记一笔：{:?}",
        out.adjustments
    );
}

#[test]
fn no_temperature_override_when_unset() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "hi")];
    let tools: [ToolSpec; 0] = [];
    let late_tools: [ToolSpec; 0] = [];
    let config = kimi_config(None);

    let ing = ingredients(&sys, &messages, &tools, &late_tools, &config, RequestIntent::Free, None);

    let out = Kimi.encode(&ing);
    assert!(
        !out.adjustments.iter().any(|a| matches!(a, Adjustment::TemperatureOverridden { .. })),
        "config.temperature 是 None 时没有值可覆盖，不该报 TemperatureOverridden：{:?}",
        out.adjustments
    );
}

// ---------------------------------------------------------------------
// 3. late_tools：Kimi 有消息级 tools 通道，零代价（§二「中途加载工具的代价」）。
// ---------------------------------------------------------------------

/// 晚加的工具不该触发 `LateToolsForcedIntoPrefix`（那是没有消息级通道的家才付
/// 的代价），且工具名字（或它的转义形式）必须真的出现在 body 字节里——
/// 记录了调整但没真的发工具定义，一样是静默妥协。
#[test]
fn late_tools_go_in_free_via_message_level_channel() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "hi")];
    let tools = [tool_spec("srv:fs/read", "read a file", serde_json::json!({"type": "object"}))];
    let late = [tool_spec("srv:fs/write", "write a file", serde_json::json!({"type": "object"}))];
    let config = kimi_config(None);

    let ing = ingredients(&sys, &messages, &tools, &late, &config, RequestIntent::Free, None);

    let out = Kimi.encode(&ing);
    assert!(
        !out.adjustments.iter().any(|a| matches!(a, Adjustment::LateToolsForcedIntoPrefix { .. })),
        "Kimi 消息级 tools 零代价，不该报 LateToolsForcedIntoPrefix：{:?}",
        out.adjustments
    );

    let body = String::from_utf8(out.body).expect("body 必须是合法 UTF-8");
    // 只断言名字片段而不是整个全名：`:` / `/` 可能被转义成 wire 收得下的字符集
    // （见 stream/mod.rs `with_name_from_wire` 的文档），"write" 这个片段不含
    // 特殊字符，任何合理的转义规则都不会碰它。
    assert!(
        body.contains("write"),
        "晚加工具的名字片段必须出现在 body 字节里（原名或其转义形），实际 body: {body}"
    );
}

// ---------------------------------------------------------------------
// 4. 流式 usage：缺失路径 → cached None；尾帧（finish 后、choices 空）→ 拿得到。
// ---------------------------------------------------------------------

/// 速查表：Kimi 未命中时 `cached_tokens` 路径整个缺失——不是「字段在、值是
/// 0」。构造一段没有 `prompt_tokens_details` 的 usage，`finish()` 出来的
/// `cached` 必须是 `None`（`TokenUsage::cached` 的文档：这俩语义不同）。
#[test]
fn stream_usage_without_cached_details_path_is_none() {
    let mut acc = Kimi.accumulator();
    acc.push_line(r#"data: {"choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}"#);
    acc.push_line(
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":42,"completion_tokens":3}}"#,
    );
    acc.push_line("data: [DONE]");

    let (_, _, usage) = acc.finish();
    assert_eq!(usage.cached, None, "usage 里没有 prompt_tokens_details 时 cached 必须是 None");
}

/// §三：Kimi 的 usage 在 finish 帧之后**另起一帧**，且那帧 `choices` 为空。
/// 假定每帧都有 `choices[0]` 的解码器要么 panic 要么丢掉这份 usage——尾帧单独
/// 喂进去必须拿得到，不能因为 `choices` 为空就跳过。
#[test]
fn stream_usage_arrives_on_trailing_empty_choices_frame() {
    let mut acc = Kimi.accumulator();
    acc.push_line(r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#);
    let events = acc.push_line(
        r#"data: {"choices":[],"usage":{"prompt_tokens":110,"completion_tokens":61,"prompt_tokens_details":{"cached_tokens":110}}}"#,
    );
    assert!(
        events.iter().any(|e| matches!(e, StreamEvent::UsageReady(_))),
        "尾帧 choices 为空但带 usage，必须吐出 UsageReady：{events:?}"
    );
    acc.push_line("data: [DONE]");

    let (_, _, usage) = acc.finish();
    assert_eq!(usage.prompt, 110);
    assert_eq!(usage.cached, Some(110));
}

// ---------------------------------------------------------------------
// 5. classify：状态码分配不一致（§四），Kimi 的模型名错误是 404。
// ---------------------------------------------------------------------

/// 404 + `resource_not_found_error` → `BadRequest`。404 在别处通常意味着不可
/// 恢复的路径问题，但这里是「模型名错误」——按 `error.type` 判，不是猜状态码。
#[test]
fn classify_404_resource_not_found_is_bad_request() {
    let body = r#"{"error":{"message":"model not found","type":"resource_not_found_error"}}"#;
    assert_eq!(Kimi.classify(404, body), ErrorClass::BadRequest);
}

/// 429 + `engine_overloaded_error` → `Retryable`。
#[test]
fn classify_429_engine_overloaded_is_retryable() {
    let body = r#"{"error":{"message":"engine overloaded","type":"engine_overloaded_error"}}"#;
    assert_eq!(Kimi.classify(429, body), ErrorClass::Retryable);
}

/// key 无效：401 → `Auth`。
#[test]
fn classify_401_is_auth() {
    let body = r#"{"error":{"message":"invalid api key","type":"invalid_authentication_error"}}"#;
    assert_eq!(Kimi.classify(401, body), ErrorClass::Auth);
}
