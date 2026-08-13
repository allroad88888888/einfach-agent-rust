//! `openai` 子模块单测共用的最小料单构造。**只在 `cfg(test)` 下编译。**
//!
//! 形状照抄 `glm/test_support.rs`，只有 model 名不同——这里刻意用一个
//! **不属于任何一家**的名字，提醒读者这个 adapter 不对应任何具体 provider。

use std::sync::{Arc, OnceLock};

use agent_core::{RequestIntent, SessionConfig, ToolSpec};
use serde_json::{Value, json};

use crate::Ingredients;

pub(crate) fn config() -> &'static SessionConfig {
    static CONFIG: OnceLock<SessionConfig> = OnceLock::new();
    CONFIG.get_or_init(|| SessionConfig {
        model: Arc::from("some-openai-compatible-model"),
        // **有意设成 Some**：最小内核契约的核心断言就是「设了也不发」，
        // 留 None 的话那条测试等于什么都没测。
        temperature: Some(0.0),
        max_tokens: None,
        context_window: None,
    })
}

/// 一份空料单：没有 system、没有历史、没有工具、意图 `Free`、冷启动。
pub(crate) fn ing() -> Ingredients<'static> {
    Ingredients {
        system: &[],
        messages: &[],
        tools: &[],
        late_tools: &[],
        config: config(),
        intent: RequestIntent::Free,
        prev_prefix: None,
    }
}

pub(crate) fn spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from("d"),
        schema: Arc::new(json!({"type": "object"})),
    }
}

/// 请求体里 `tools[*].function.name` 的列表（wire 形状的名字）。
pub(crate) fn tool_names(body: &Value) -> Vec<String> {
    body.get("tools")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}
