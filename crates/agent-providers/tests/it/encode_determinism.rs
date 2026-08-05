//! 红线 11：会进 prompt 的东西，序列化必须逐字节确定。
//!
//! 覆盖两层：
//! 1. 同一份 `Ingredients` 两次调 `encode`，`body` 必须逐字节相同。
//! 2. 两份「值相等但插入顺序不同」的工具 schema（`serde_json::Value` 的
//!    `Map` 后端是 `BTreeMap`，红线 11 依赖的正是这个默认行为），组出的两份
//!    料单序列化后也必须逐字节相同——证明 adapter 没有引入任何顺序敏感的
//!    序列化路径（比如中途转成 HashMap 再迭代）。

mod support;

use agent_core::RequestIntent;
use agent_providers::Provider;

/// 料单要有料：≥2 条 system chunk、≥2 条消息、≥2 个工具。
fn rich_system() -> Vec<agent_core::SystemChunk> {
    vec![
        support::sys_chunk("base", "你是一个称职的助手。"),
        support::sys_chunk("skill:fs", "读写文件时先确认路径存在。"),
    ]
}

fn rich_messages() -> Vec<agent_core::Message> {
    vec![
        support::user_text(1, "帮我看看北京天气"),
        support::assistant_text(2, "好的，我先查一下。"),
    ]
}

#[test]
fn same_ingredients_encode_twice_byte_identical() {
    let provider = support::provider();
    let system = rich_system();
    let messages = rich_messages();
    let tools = vec![
        support::tool_spec("srv:fs/read", "read a file", support::schema_order_a()),
        support::tool_spec(
            "srv:fs/write",
            "write a file",
            serde_json::json!({"type": "object"}),
        ),
    ];
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

    let a = provider.encode(&ing);
    let b = provider.encode(&ing);
    assert_eq!(
        a.body, b.body,
        "同一份 Ingredients 两次 encode 必须逐字节相同"
    );
}

#[test]
fn tool_schema_key_order_does_not_affect_encoded_bytes() {
    let provider = support::provider();
    let system = rich_system();
    let messages = rich_messages();
    let config = support::session_config();
    let late_tools: Vec<agent_core::ToolSpec> = vec![];

    // 两份工具表：schema 里的 key 集合与取值完全相同，只是构造 Map 时的插入
    // 顺序相反。
    let tools_a = vec![
        support::tool_spec("srv:fs/read", "read a file", support::schema_order_a()),
        support::tool_spec(
            "srv:fs/list",
            "list a dir",
            serde_json::json!({"type": "object"}),
        ),
    ];
    let tools_b = vec![
        support::tool_spec("srv:fs/read", "read a file", support::schema_order_b()),
        support::tool_spec(
            "srv:fs/list",
            "list a dir",
            serde_json::json!({"type": "object"}),
        ),
    ];

    let ing_a = support::ingredients(
        &system,
        &messages,
        &tools_a,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );
    let ing_b = support::ingredients(
        &system,
        &messages,
        &tools_b,
        &late_tools,
        &config,
        RequestIntent::Free,
        None,
    );

    let encoded_a = provider.encode(&ing_a);
    let encoded_b = provider.encode(&ing_b);
    assert_eq!(
        encoded_a.body, encoded_b.body,
        "schema 的 key 插入顺序不同，但值相等，序列化出的 body 必须逐字节相同"
    );
}
