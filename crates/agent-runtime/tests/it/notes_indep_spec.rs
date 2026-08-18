//! 209：工具说明书本身也是交付物（同 `send_indep_spec.rs` 那条道理）——`key`
//! 是必填、`with_notes()` 一档开两个工具、且只加不改既有前缀（红线 11）。
//!
//! 断的是**schema 形状**，不是文案正文：这份独立测试没读过 `notes_tool.rs`，
//! 猜文案原话只会写出脆弱又可能猜错的断言（206 的教训）。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「做什么」2（`{ key, value }`，
//! `value` 为 `null`/空 → 删）、docs/INVARIANTS.md 红线 11、以及公开签名
//! `NOTES_TOOL` / `NOTES_SET_TOOL` / `notes_spec()` / `notes_set_spec()` /
//! `ToolTable::with_notes()`。**实现体一行没读**：`notes_tool.rs`、
//! `notes_render.rs` 及各自的 `_tests.rs` 全程没打开。

use agent_core::AgentLimits;
use agent_runtime::{NOTES_SET_TOOL, NOTES_TOOL, ToolTable, notes_set_spec, notes_spec};

#[test]
fn the_tool_names_are_the_documented_srv_agent_paths() {
    assert_eq!(NOTES_TOOL, "srv:agent/notes");
    assert_eq!(NOTES_SET_TOOL, "srv:agent/notes/set");
    assert_eq!(&*notes_spec().name, NOTES_TOOL);
    assert_eq!(&*notes_set_spec().name, NOTES_SET_TOOL);
}

/// 读工具**无入参**——跟 `self_spec()`/`status_spec()` 同一个形状：自己是谁
/// 由截获现场的 `AgentId` 决定，不给模型一个能填错的口。
#[test]
fn the_read_tool_takes_no_input() {
    let schema = &*notes_spec().schema;
    assert_eq!(
        schema["type"], "object",
        "schema 顶层该是个 object：{schema}"
    );
    let required = schema.get("required").and_then(|r| r.as_array());
    assert!(
        required.is_none_or(|r| r.is_empty()),
        "读工具不该要求任何入参：{schema}"
    );
}

/// 写工具的入参是 `{ key, value }`，`key` 必填——`value` 缺省/`null` 是「删掉
/// 这条」的合法输入，不该被 schema 的 `required` 挡在门外，所以这里**不**断言
/// `value` 必填（那是猜实现细节，猜错就是一条假红）。
#[test]
fn the_set_tool_schema_takes_key_and_value_with_key_required() {
    let spec = notes_set_spec();
    let schema = &*spec.schema;

    let props = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("schema 该有 properties：{schema}"));
    for field in ["key", "value"] {
        assert!(props.contains_key(field), "入参该有 {field}：{schema}");
    }

    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap_or_else(|| panic!("schema 该有 required：{schema}"))
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"key"), "`key` 是必填：{schema}");
}

/// 红线 11：进 prompt 的东西逐字节确定——两次调用拿到的描述与 schema 序列化
/// 必须一样，不许带时间戳/随机 id。
#[test]
fn both_specs_are_byte_identical_across_two_calls() {
    for (a, b) in [
        (notes_spec(), notes_spec()),
        (notes_set_spec(), notes_set_spec()),
    ] {
        assert_eq!(a.description, b.description, "描述逐字节确定");
        assert_eq!(
            serde_json::to_string(&*a.schema).unwrap(),
            serde_json::to_string(&*b.schema).unwrap(),
            "schema 序列化逐字节确定"
        );
    }
}

/// `with_notes()` 一档开两个工具（照模块文档「一档是一件事」的先例，同
/// `with_skills` 的 read+index 一起），**只加不改**：既有工具的顺序一个字节不动，
/// 新增的两个按 `notes.rs::with_notes` 声明的顺序追加在末尾。
#[test]
fn with_notes_appends_both_tools_without_disturbing_the_existing_prefix() {
    let before: Vec<String> = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .specs()
        .iter()
        .map(|s| s.name.to_string())
        .collect();
    let after: Vec<String> = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_notes()
        .specs()
        .iter()
        .map(|s| s.name.to_string())
        .collect();

    assert_eq!(
        after[..before.len()],
        before[..],
        "既有工具的顺序一个字节不该动（工具表在 prompt 最前面）"
    );
    assert_eq!(
        &after[before.len()..],
        &[NOTES_TOOL.to_string(), NOTES_SET_TOOL.to_string()],
        "两个新工具该按声明顺序追加在末尾：{after:?}"
    );
}

/// 不开 `with_notes` 就不该声明这两个工具——每个 `with_*` 是一档独立授权，
/// 不是「多 agent 模式」的总开关。
#[test]
fn without_with_notes_neither_tool_is_declared() {
    let table = ToolTable::builtin();
    assert!(!table.declares(NOTES_TOOL));
    assert!(!table.declares(NOTES_SET_TOOL));
}

/// 反过来：开了就两个都声明，不会只声明一半。
#[test]
fn with_notes_declares_both_tools_together() {
    let table = ToolTable::builtin().with_notes();
    assert!(table.declares(NOTES_TOOL));
    assert!(table.declares(NOTES_SET_TOOL));
}
