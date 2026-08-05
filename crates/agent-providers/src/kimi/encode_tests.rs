//! Kimi 请求编码的行为测试。

use std::sync::Arc;

use agent_core::{Adjustment, RequestIntent, Segment, SystemChunk};
use serde_json::{Value, json};

use super::encode::encode;
use super::test_support::{ing, spec};

/// 红线 11：同一份料两次 encode 逐字节相同。
#[test]
fn same_ingredients_encode_byte_identical() {
    let t = [spec("srv:fs/read"), spec("srv:fs/write")];
    let s = [SystemChunk {
        label: Arc::from("base"),
        text: Arc::from("你是助手"),
    }];
    let mut a = ing();
    a.tools = &t;
    a.system = &s;
    assert_eq!(encode(&a).body, encode(&a).body);
}

/// `Free` 不带 tool_choice，也不产生任何 Adjustment。
#[test]
fn free_intent_sends_no_tool_choice() {
    let out = encode(&ing());
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert!(body.get("tool_choice").is_none());
    assert!(out.adjustments.is_empty());
}

/// `MustUseTool` 直接可用，无调整。
#[test]
fn must_use_tool_needs_no_adjustment() {
    let mut i = ing();
    i.intent = RequestIntent::MustUseTool;
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["tool_choice"], json!("required"));
    assert!(out.adjustments.is_empty());
}

/// temperature：`None` 不传不调整；恰好 1.0 原样传不调整；其余钳成 1.0 并报调整。
#[test]
fn temperature_only_accepts_one() {
    let out = encode(&ing());
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert!(body.get("temperature").is_none());
    assert!(out.adjustments.is_empty());

    let mut cfg = super::test_support::config().clone();
    cfg.temperature = Some(1.0);
    let mut i = ing();
    i.config = &cfg;
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["temperature"], json!(1.0));
    assert!(out.adjustments.is_empty());

    cfg.temperature = Some(0.3);
    let mut i = ing();
    i.config = &cfg;
    let out = encode(&i);
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["temperature"], json!(1.0));
    assert_eq!(
        out.adjustments,
        vec![Adjustment::TemperatureOverridden {
            wanted: 0.3,
            used: 1.0
        }]
    );
}

/// 中途加载的工具追加成消息级，顶层 `tools` 不变；不产生调整。
#[test]
fn late_tools_go_message_level_not_top_level() {
    let t = [spec("srv:fs/read")];
    let late = [spec("srv:late/a")];
    let mut i = ing();
    i.tools = &t;
    i.late_tools = &late;
    let out = encode(&i);
    assert!(out.adjustments.is_empty(), "{:?}", out.adjustments);

    let body: Value = serde_json::from_slice(&out.body).unwrap();
    let top_names: Vec<_> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(top_names, vec!["srv_3Afs_2Fread"]);

    let messages = body["messages"].as_array().unwrap();
    let tail = messages.last().unwrap();
    assert_eq!(tail["role"], json!("system"));
    assert!(
        tail.get("content").is_none(),
        "late tools 消息不该有 content 字段"
    );
    assert_eq!(
        tail["tools"][0]["function"]["name"],
        json!("srv_3Alate_2Fa")
    );
}

/// 上一轮的 History 只是被延长（追加了 late tools 消息），前缀不算漂。
#[test]
fn late_tools_message_is_a_strict_extension_of_prior_history() {
    let mut first = ing();
    let cold = encode(&first);
    let mut prev = cold.prefix;
    prev.prompt_tokens = Some(5120);

    let late = [spec("srv:late/a")];
    first.late_tools = &late;
    first.prev_prefix = Some(&prev);
    let out = encode(&first);
    assert_eq!(out.drift, None, "追加消息级 tools 不该判漂");
    assert_eq!(out.predicted_cache, 5120 / 256 * 256);
}

/// `MustUse(name)` 无条件降级，即使工具就在料单里也不尝试指定函数。
#[test]
fn must_use_named_tool_is_unconditionally_downgraded() {
    let t = [spec("srv:fs/read")];
    let mut i = ing();
    i.tools = &t;
    i.intent = RequestIntent::MustUse(Arc::from("srv:fs/read"));
    let out = encode(&i);
    assert_eq!(
        out.adjustments,
        vec![Adjustment::ToolChoiceDowngraded {
            wanted: Arc::from("srv:fs/read"),
            used: Arc::from("required"),
        }]
    );
    let body: Value = serde_json::from_slice(&out.body).unwrap();
    assert_eq!(body["tool_choice"], json!("required"));
}

/// 冷启动不算漂；换掉 system 后漂在 System 段。
#[test]
fn drift_points_at_the_changed_segment() {
    let s1 = [SystemChunk {
        label: Arc::from("base"),
        text: Arc::from("一"),
    }];
    let s2 = [SystemChunk {
        label: Arc::from("base"),
        text: Arc::from("二"),
    }];
    let mut first = ing();
    first.system = &s1;
    let cold = encode(&first);
    assert_eq!((cold.drift, cold.predicted_cache), (None, 0));

    let mut prev = cold.prefix;
    prev.prompt_tokens = Some(700);
    let mut second = ing();
    second.system = &s2;
    second.prev_prefix = Some(&prev);
    let out = encode(&second);
    assert_eq!((out.drift, out.predicted_cache), (Some(Segment::System), 0));
}
