//! GLM adapter 的黑盒验收测试（issue 023）。
//!
//! 独立测试 agent 规则：只依据 `docs/issues/023-three-providers.md` 的验收条目、
//! `probes/PROVIDERS.md` 的实测数据、`docs/INVARIANTS.md` 红线 11/12、
//! `docs/ADAPTER.md` 的接缝定义，以及 `agent-providers` / `agent-core` 的公开签名。
//! **不看** `crates/agent-providers/src/glm/` 的实现体。

use std::sync::Arc;

use agent_core::{Adjustment, ContentBlock, RequestIntent, SessionConfig, ToolSpec};
use agent_providers::Provider;
use agent_providers::glm::Glm;

use crate::support::{ingredients, sys_chunk, tool_spec, user_text};

fn glm_config(temperature: Option<f32>) -> SessionConfig {
    SessionConfig {
        model: Arc::from("glm-5.2"),
        temperature,
        max_tokens: Some(4096),
        context_window: Some(128_000),
    }
}

// ---------------------------------------------------------------------
// 1. §二速查表：GLM 开/关思考都能直接指定函数——不该降级，也不该关思考。
//    （文档说只支持 auto，实测四种全支持——以 PROVIDERS.md 实测为准。）
// ---------------------------------------------------------------------

#[test]
fn must_use_named_tool_is_native_no_adjustments() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "read the file")];
    let tools = [tool_spec(
        "srv:fs/read",
        "read a file",
        serde_json::json!({"type": "object"}),
    )];
    let late_tools: [ToolSpec; 0] = [];
    let config = glm_config(None);

    let ing = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::MustUse(Arc::from("srv:fs/read")),
        None,
    );

    let out = Glm.encode(&ing);
    assert!(
        !out.adjustments
            .iter()
            .any(|a| matches!(a, Adjustment::ToolChoiceDowngraded { .. })),
        "GLM 原生支持指定函数，不该降级：{:?}",
        out.adjustments
    );
    assert!(
        !out.adjustments
            .iter()
            .any(|a| matches!(a, Adjustment::ThinkingDisabledForToolChoice)),
        "GLM 思考可开关，不需要为满足 tool_choice 关掉：{:?}",
        out.adjustments
    );
}

// ---------------------------------------------------------------------
// 2. §二「中途加载工具的代价」：GLM 只能并进顶层，全价重编码，折扣比 2x。
// ---------------------------------------------------------------------

#[test]
fn late_tools_force_full_prefix_rebuild_at_2x() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "hi")];
    let tools = [tool_spec(
        "srv:fs/read",
        "read a file",
        serde_json::json!({"type": "object"}),
    )];
    let late = [tool_spec(
        "srv:fs/write",
        "write a file",
        serde_json::json!({"type": "object"}),
    )];
    let config = glm_config(None);

    let ing = ingredients(
        &sys,
        &messages,
        &tools,
        &late,
        &config,
        RequestIntent::Free,
        None,
    );

    let out = Glm.encode(&ing);
    let est_cost_multiple = out.adjustments.iter().find_map(|a| match a {
        Adjustment::LateToolsForcedIntoPrefix {
            est_cost_multiple, ..
        } => Some(*est_cost_multiple),
        _ => None,
    });
    assert_eq!(
        est_cost_multiple,
        Some(2.0),
        "GLM 晚加工具只能并进顶层，代价 2x：{:?}",
        out.adjustments
    );
}

// ---------------------------------------------------------------------
// 3. 速查表：GLM 未命中时字段在、值为 0——跟 Kimi「整个缺失」不是一回事，
//    这是本 issue 最容易混的一对。
// ---------------------------------------------------------------------

#[test]
fn stream_usage_cached_zero_is_some_zero_not_none() {
    let mut acc = Glm.accumulator();
    acc.push_line(
        r#"data: {"choices":[{"index":0,"delta":{"content":"好"},"finish_reason":null}]}"#,
    );
    acc.push_line(
        r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":900,"completion_tokens":10,"prompt_tokens_details":{"cached_tokens":0}}}"#,
    );
    acc.push_line("data: [DONE]");

    let (_, _, usage) = acc.finish();
    assert_eq!(
        usage.cached,
        Some(0),
        "cached_tokens: 0 必须解析成 Some(0)，不是 None"
    );
}

// ---------------------------------------------------------------------
// 4. §三：GLM 每帧重复 role:"assistant"，累积时忽略，不污染文本。
// ---------------------------------------------------------------------

#[test]
fn repeated_role_field_does_not_pollute_text() {
    let mut acc = Glm.accumulator();
    for frame in [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"你"}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"好"}}]}"#,
    ] {
        acc.push_line(frame);
    }
    acc.push_line(r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#);
    acc.push_line("data: [DONE]");

    let (blocks, ..) = acc.finish();
    assert_eq!(
        blocks,
        vec![ContentBlock::Text(Arc::from("你好"))],
        "每帧重复的 role 字段不该被当成内容，也不该打断累积"
    );
}

// ---------------------------------------------------------------------
// 5. 速查表：temperature 自由，不该覆盖。
// ---------------------------------------------------------------------

#[test]
fn temperature_is_free_no_override() {
    let sys = [sys_chunk("base", "sys")];
    let messages = [user_text(1, "hi")];
    let tools: [ToolSpec; 0] = [];
    let late_tools: [ToolSpec; 0] = [];
    let config = glm_config(Some(0.7));

    let ing = ingredients(
        &sys,
        &messages,
        &tools,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );

    let out = Glm.encode(&ing);
    assert!(
        !out.adjustments
            .iter()
            .any(|a| matches!(a, Adjustment::TemperatureOverridden { .. })),
        "GLM 温度自由，传什么就是什么，不该覆盖：{:?}",
        out.adjustments
    );
}
