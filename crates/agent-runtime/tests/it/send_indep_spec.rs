//! 206：**工具说明书本身也是交付物**。模型只能看见 `send_spec()` 那段文字，
//! 两档的差别、`next_turn` 只能发给 root、以及「对方已经答完就没人读」这三件事
//! 说不清楚，前面所有的行为断言在真机上都换不来正确的调用。
//!
//! 断的是**关键子串与 schema 形状**，不是整段文案——文案改一个字这里不该跟着红。
//!
//! 顺带钉红线 11：工具表在 prompt 最前面，`send_spec()` 必须逐字节确定
//! （没有时间戳、没有随机排序），而且 `with_send()` 是**只加不改**：既有工具的
//! 顺序一个字节不动。
//!
//! 黑盒来源与「实现体没读」的声明见 `send_indep_support/mod.rs` 顶部。

use agent_core::AgentLimits;
use agent_runtime::{SEND_TOOL, ToolTable, send_spec};

#[test]
fn the_send_tool_description_tells_the_model_the_three_things_it_must_know() {
    let spec = send_spec();
    assert_eq!(&*spec.name, SEND_TOOL);
    assert_eq!(SEND_TOOL, "srv:agent/send", "工具名是对外契约");

    let d = spec.description.to_string();

    // ① 两档的差别：两个档位的字面值都得出现，而且要说到「下一轮」这层时间差。
    for needle in ["now", "next_turn", "下一轮"] {
        assert!(
            d.contains(needle),
            "描述里该出现 {needle}——模型是靠这段文字选档的：{d}"
        );
    }

    // ② `next_turn` 只能发给 root。
    assert!(
        d.contains("root"),
        "描述该点名下一轮唯一活着的收件箱是 root：{d}"
    );

    // ③ 「对方已经答完就没人读」——不说，模型会以为发出去必然被看见。
    // 候选里**刻意不放「读到」**：那两个字在「`when` 决定它什么时候被对方读到」
    // 这种中性句里也成立，放进来这条断言就变成一句永远绿的废话。
    assert!(
        ["答完", "终态", "没人读", "读不到"]
            .iter()
            .any(|n| d.contains(n)),
        "描述该说清「收信人已经答完的话这条就没人读」：{d}"
    );
}

#[test]
fn the_send_schema_takes_to_text_and_an_optional_two_valued_when() {
    let spec = send_spec();
    let schema = &*spec.schema;

    let props = schema["properties"]
        .as_object()
        .unwrap_or_else(|| panic!("schema 该有 properties：{schema}"));
    for key in ["to", "text", "when"] {
        assert!(props.contains_key(key), "入参该有 {key}：{schema}");
    }

    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap_or_else(|| panic!("schema 该有 required：{schema}"))
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"to"), "`to` 是必填：{schema}");
    assert!(required.contains(&"text"), "`text` 是必填：{schema}");
    assert!(
        !required.contains(&"when"),
        "`when` 缺省是 now，不该是必填：{schema}"
    );

    let variants: Vec<&str> = props["when"]["enum"]
        .as_array()
        .unwrap_or_else(|| panic!("`when` 该是个两值枚举：{schema}"))
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        variants,
        vec!["now", "next_turn"],
        "只有两档，且顺序进 prompt——红线 11：{schema}"
    );
}

/// 红线 11：进 prompt 的东西逐字节确定；`with_send()` 只加不改。
#[test]
fn with_send_appends_the_tool_without_disturbing_the_existing_prefix() {
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
        .with_send()
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
        after.last().map(String::as_str),
        Some(SEND_TOOL),
        "send 该追加在末尾：{after:?}"
    );
    assert!(
        !ToolTable::builtin().declares(SEND_TOOL),
        "不开 `with_send` 就不该声明它——每个 with_* 是一档独立授权"
    );

    let a = send_spec();
    let b = send_spec();
    assert_eq!(a.description, b.description, "描述逐字节确定（红线 11）");
    assert_eq!(
        serde_json::to_string(&*a.schema).unwrap(),
        serde_json::to_string(&*b.schema).unwrap(),
        "schema 序列化逐字节确定（红线 11）"
    );
}
