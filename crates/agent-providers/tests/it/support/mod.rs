//! 测试专用的料单构造助手。只负责把 `Ingredients` 需要的各种值类型拼起来，
//! 不含任何断言——断言留给各个测试文件自己写。
//!
//! 独立测试 agent 按 issue 025 的规则只看公开签名和 docs/ADAPTER.md、
//! probes/PROVIDERS.md，不看 `crates/agent-providers/src/deepseek/` 的实现。

#![allow(dead_code)]

use std::sync::Arc;

use agent_core::{
    ContentBlock, Message, MessageId, PrefixImage, Role, SessionConfig, SystemChunk, ToolSpec,
};
use agent_providers::Ingredients;
use agent_providers::deepseek::DeepSeek;
use serde_json::Value;

/// `DeepSeek` 是无状态的（issue 025：adapter 全部方法是纯函数），每个测试独立构造一份。
pub fn provider() -> DeepSeek {
    DeepSeek
}

pub fn sys_chunk(label: &str, text: &str) -> SystemChunk {
    SystemChunk {
        label: Arc::from(label),
        text: Arc::from(text),
    }
}

pub fn user_text(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

pub fn assistant_text(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

pub fn tool_spec(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(schema),
    }
}

pub fn session_config() -> SessionConfig {
    SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: Some(0.7),
        max_tokens: Some(4096),
        context_window: Some(128_000),
    }
}

/// 组一份 `Ingredients`，各字段的生命周期都来自调用方已经拥有的切片/值，
/// 这个函数本身不拥有任何数据（`Ingredients` 是纯引用结构）。
pub fn ingredients<'a>(
    system: &'a [SystemChunk],
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    late_tools: &'a [ToolSpec],
    config: &'a SessionConfig,
    intent: agent_core::RequestIntent,
    prev_prefix: Option<&'a PrefixImage>,
) -> Ingredients<'a> {
    // 039 的 `late_system` 走独立的 `skill_indep_late_system_placement.rs`（它直接
    // 构造 `Ingredients` 字面量），这个共用 builder 的既有调用方都不带 skill 注入，
    // 所以这里硬编码空——加成参数会波及 32 个调用点却没有一个真的用它。
    Ingredients {
        system,
        messages,
        tools,
        late_tools,
        late_system: &[],
        config,
        intent,
        prev_prefix,
    }
}

/// 两个不常用工具的 schema，用两种不同 key 插入顺序构造出「值相等」的
/// `serde_json::Value`——验证 `Map` 是 `BTreeMap` 而不是插入顺序敏感的容器
/// （红线 11 的机制，agent-core 的 `tool.rs` 已经证过一遍，这里在料单层面复证）。
pub fn schema_order_a() -> Value {
    let mut map = serde_json::Map::new();
    map.insert("path".to_string(), serde_json::json!({"type": "string"}));
    map.insert(
        "recursive".to_string(),
        serde_json::json!({"type": "boolean"}),
    );
    map.insert(
        "encoding".to_string(),
        serde_json::json!({"type": "string"}),
    );
    Value::Object(map)
}

pub fn schema_order_b() -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "encoding".to_string(),
        serde_json::json!({"type": "string"}),
    );
    map.insert(
        "recursive".to_string(),
        serde_json::json!({"type": "boolean"}),
    );
    map.insert("path".to_string(), serde_json::json!({"type": "string"}));
    Value::Object(map)
}
