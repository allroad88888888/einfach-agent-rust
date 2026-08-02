//! 各子模块单测共用的最小料单构造。**只在 `cfg(test)` 下编译。**
//!
//! 单独一个文件是因为 `Ingredients` 的引用字段要一个 `'static` 的 config，
//! 每个模块各造一份既啰嗦又容易造得不一样，比对逐字节相等时就没意义了。

use std::sync::{Arc, OnceLock};

use agent_core::{RequestIntent, SessionConfig, ToolSpec};
use serde_json::{Value, json};

use crate::Ingredients;

pub(crate) fn config() -> &'static SessionConfig {
    static CONFIG: OnceLock<SessionConfig> = OnceLock::new();
    CONFIG.get_or_init(|| SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
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
