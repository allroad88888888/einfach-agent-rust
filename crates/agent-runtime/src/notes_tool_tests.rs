//! `notes_spec()` / `notes_set_spec()` 那两段**说明书**，以及入参解析。
//!
//! 端到端（真的写、真的 undo、真的恢复）在 `tests/it/notes_indep_*.rs`。

use super::*;

use serde_json::json;

/// 读那一半：全名、无入参、说清「只有你看得见」和「不会自动出现」。
///
/// 后一句是承重的：草稿纸**不自动注入进 prompt**（否则每次写都打掉前缀缓存，
/// 红线 11）。不说清，模型会以为记下的东西下一轮自己会看到。
#[test]
fn the_read_spec_says_it_is_private_and_not_automatic() {
    let spec = notes_spec();
    assert_eq!(&*spec.name, NOTES_TOOL);
    assert_eq!(spec.schema["properties"], json!({}), "读不该有任何入参");

    let text = &*spec.description;
    assert!(text.contains("只有你看得见"), "{text}");
    assert!(text.contains("不会自动出现"), "{text}");
    assert!(
        text.contains(crate::SEND_TOOL),
        "要给别人传话是另一个工具，得点名：{text}"
    );
}

/// 写那一半：覆盖语义、删的写法、两个上限的数字。
///
/// **数字必须跟真正拦人的是同一个**（同 `with_spawn` 那条既有耦合）：
/// 描述里写 512 而闸是 1024，模型会白白把内容切一半。
#[test]
fn the_set_spec_states_the_same_caps_that_actually_stop_it() {
    let spec = notes_set_spec();
    assert_eq!(&*spec.name, NOTES_SET_TOOL);

    let text = &*spec.description;
    assert!(text.contains("覆盖"), "同名是覆盖不是追加：{text}");
    assert!(text.contains("null"), "删的写法得说：{text}");

    let key_doc = spec.schema["properties"]["key"]["description"]
        .as_str()
        .expect("key 得有描述");
    assert!(
        key_doc.contains(&agent_core::NOTE_KEY_CAP.to_string()),
        "key 上限的数字得是真正拦人的那个：{key_doc}"
    );
    let value_doc = spec.schema["properties"]["value"]["description"]
        .as_str()
        .expect("value 得有描述");
    assert!(
        value_doc.contains(&NOTE_VALUE_CAP.to_string()),
        "value 上限的数字得是真正拦人的那个：{value_doc}"
    );
    assert_eq!(spec.schema["required"], json!(["key"]), "只有 key 是必填");
}

/// 缺省与显式 `null` 是同一件事：删掉这条。模型两种都会写。
#[test]
fn a_missing_value_and_an_explicit_null_both_mean_delete() {
    let (_, missing, _) = parse(&json!({"key": "k"})).expect("解析");
    let (_, null, _) = parse(&json!({"key": "k", "value": null})).expect("解析");
    assert!(missing.is_none());
    assert!(null.is_none());
}

/// key 两头的空白剪掉——模型写 `" plan"` 和 `"plan"` 该是同一条，
/// 不然它下一轮查不到自己刚记的东西。
#[test]
fn a_key_is_trimmed() {
    let (key, _, _) = parse(&json!({"key": "  plan  ", "value": "x"})).expect("解析");
    assert_eq!(&*key, "plan");
}

/// 超长正文**在这一层截断**，并把原始长度带出来给回执用。
#[test]
fn an_oversized_value_is_truncated_and_the_original_size_comes_back() {
    let long = "x".repeat(NOTE_VALUE_CAP + 500);
    let (_, value, truncated) = parse(&json!({"key": "k", "value": long})).expect("解析");
    let value = value.expect("不是删");
    assert_eq!(value.len(), NOTE_VALUE_CAP);
    assert_eq!(truncated, Some(NOTE_VALUE_CAP + 500));
}

/// 截断必须落在 UTF-8 字符边界上。按字节硬切会把一个中文字劈成半个——
/// 那个值会进状态、进 prompt，直到序列化时才炸。
#[test]
fn truncation_never_splits_a_multibyte_character() {
    // 每个中文字 3 字节，1024 不是 3 的倍数 → 边界必然落在字符中间。
    let long = "字".repeat(NOTE_VALUE_CAP);
    let (_, value, _) = parse(&json!({"key": "k", "value": long})).expect("解析");
    let value = value.expect("不是删");
    assert!(value.len() <= NOTE_VALUE_CAP);
    assert_eq!(
        value.len() % 3,
        0,
        "切在了字符中间：{} 字节不是 3 的倍数",
        value.len()
    );
}

/// 恰好等于上限不该被当成超限（差一错的经典落点）。
#[test]
fn a_value_exactly_at_the_cap_is_not_truncated() {
    let exact = "x".repeat(NOTE_VALUE_CAP);
    let (_, value, truncated) = parse(&json!({"key": "k", "value": exact})).expect("解析");
    assert_eq!(value.expect("不是删").len(), NOTE_VALUE_CAP);
    assert_eq!(truncated, None, "没截就不该说截了");
}

/// 入参写错一律回给模型看的文本，不 panic（003 的哲学）。
#[test]
fn bad_input_becomes_a_message_for_the_model() {
    for bad in [
        json!({}),
        json!({"key": ""}),
        json!({"key": "   "}),
        json!({"key": 3}),
        json!({"key": "k", "value": 3}),
        json!({"key": "k", "value": ["a"]}),
    ] {
        let err = parse(&bad).expect_err("该被拒");
        assert!(err.starts_with("记笔记失败："), "{bad} → {err}");
    }
}
