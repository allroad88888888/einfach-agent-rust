//! `openai::encode` 的单测。核心是钉住**没发什么**——最小内核契约（175 决策二）
//! 靠的正是这些「不存在」的断言，一条没有就等于契约没被守住。

use super::*;
use crate::openai::test_support::{ing, spec, tool_names};
use agent_core::{RequestIntent, SystemChunk};
use std::sync::Arc;

/// 红线 11：同一份料两次 encode 逐字节相同。
#[test]
fn same_ingredients_encode_byte_identical() {
    let t = [spec("srv:fs/read"), spec("srv:fs/write")];
    let s = [SystemChunk {
        label: Arc::from("base"),
        text: Arc::from("You are an assistant."),
    }];
    let mut a = ing();
    a.tools = &t;
    a.system = &s;
    assert_eq!(encode(&a).body, encode(&a).body);
}

/// **契约的核心断言**：`config.temperature` 是 `Some(0.0)`，但请求体里没有它。
///
/// 174 实测 Kimi 对 `temperature: 0.0` 直接 400（0.0 是 OpenAI 的合法值），
/// 而只发最小内核时三家全过。这条测试就是那个实测结论的守门人。
#[test]
fn temperature_is_never_sent_even_when_the_session_asks_for_one() {
    let i = ing();
    assert_eq!(
        i.config.temperature,
        Some(0.0),
        "料单里必须真的设了 temperature，否则这条测试测了个寂寞"
    );
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert!(body.get("temperature").is_none(), "最小内核契约：不发 temperature");
}

/// 丢弃 temperature **不报 `Adjustment`**：那是契约边界不是运行时妥协。
/// 每轮报一条会把 `Adjustment` 变成噪音，而它的全部价值在于稀有
/// （决策 17：「空的时候才叫原样执行了」）。
#[test]
fn dropping_temperature_does_not_emit_an_adjustment() {
    let out = encode(&ing());
    assert!(out.adjustments.is_empty());
}

/// 其余可选字段一律不发。`n` 尤其要钉住——174 实测它在 DeepSeek 上 400 拒绝、
/// 在 GLM 上 **200 静默按 1 处理**，而静默降级比拒绝更糟。
#[test]
fn optional_sampling_fields_are_never_sent() {
    let out = encode(&ing());
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    for field in ["top_p", "n", "stream_options", "presence_penalty", "thinking"] {
        assert!(body.get(field).is_none(), "最小内核不该发 {field}");
    }
}

/// 最小内核**必须**有的那几个。
#[test]
fn the_minimal_core_fields_are_all_present() {
    let mut i = ing();
    let t = [spec("srv:fs/read")];
    i.tools = &t;
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["model"], json!("some-openai-compatible-model"));
    assert!(body["messages"].is_array());
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["tools"][0]["type"], json!("function"));
}

/// `Free` 不带 `tool_choice`，也不产生任何 Adjustment。
#[test]
fn free_intent_sends_no_tool_choice() {
    let out = encode(&ing());
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert!(body.get("tool_choice").is_none());
    assert!(out.adjustments.is_empty());
}

/// `tool_choice` **属于**最小内核：它是语义要求不是采样偏好，不发就等于把
/// 「必须调工具」悄悄降级成「随你」——那是静默妥协。
#[test]
fn tool_choice_is_part_of_the_minimal_core() {
    let t = [spec("srv:fs/read")];

    let mut i = ing();
    i.tools = &t;
    i.intent = RequestIntent::MustUseTool;
    let body: Value = serde_json::from_slice(&encode(&i).body).unwrap();
    assert_eq!(body["tool_choice"], json!("required"));

    let mut i = ing();
    i.tools = &t;
    i.intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["tool_choice"]["type"], json!("function"));
    // wire 名（`srv:fs/read` → `srv_3Afs_2Fread`），跟 tools 数组里的名字一致。
    assert_eq!(
        body["tool_choice"]["function"]["name"],
        json!(tool_names(&body)[0])
    );
    assert!(
        out.adjustments.is_empty(),
        "从不截断，所以 MustUse 永远不会因为「目标被我们裁掉了」而降级"
    );
}

/// **从不截断工具**：上限未知，宁可让对面报错（可见的失败），也不自己先裁
/// （静默——模型会发现工具「不见了」，而没有任何地方说过它被丢了）。
#[test]
fn tools_are_never_truncated_by_us() {
    let many: Vec<_> = (0..300).map(|n| spec(&format!("srv:t{n}"))).collect();
    let mut i = ing();
    i.tools = &many;
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(tool_names(&body).len(), 300);
    assert!(
        out.adjustments.is_empty(),
        "没截断就不该有 ToolsTruncated"
    );
}

/// 缓存**恒不预测**：对面的块粒度与门槛未知，瞎猜一个数去跟真实 usage 对账
/// 只会制造假告警。第 2 层兜底按「无预测不判」处理。
#[test]
fn cache_is_never_predicted() {
    let s = [SystemChunk {
        label: Arc::from("base"),
        // 够长，长到在三家任何一家都早就过了起效门槛。
        text: Arc::from("x".repeat(20_000)),
    }];
    let mut i = ing();
    i.system = &s;
    let first = encode(&i);
    let mut second_in = ing();
    second_in.system = &s;
    second_in.prev_prefix = Some(&first.prefix);
    assert_eq!(encode(&second_in).predicted_cache, 0);
}

/// 晚加工具仍然报 `LateToolsForcedIntoPrefix`——前缀被作废这件事必须让人看见。
/// 但代价倍数填 1.0，意思是「我们不知道」，不是「它很便宜」。
#[test]
fn late_tools_still_report_the_prefix_invalidation() {
    let late = [spec("srv:late/one")];
    let mut i = ing();
    i.late_tools = &late;
    let out = encode(&i);
    match out.adjustments.as_slice() {
        [agent_core::Adjustment::LateToolsForcedIntoPrefix { count, .. }] => {
            assert_eq!(*count, 1);
        }
        other => panic!("该只有一条 LateToolsForcedIntoPrefix，实际 {other:?}"),
    }
}
