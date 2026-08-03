//! 各子模块单测共用的最小料单构造。**只在 `cfg(test)` 下编译。**
//!
//! 跟 `deepseek::test_support` 同一个理由分文件：`Ingredients` 的引用字段要一个
//! `'static` 的 config，各模块各造一份既啰嗦又容易造得不一样。

use std::sync::{Arc, OnceLock};

use agent_core::{RequestIntent, SessionConfig, ToolSpec};
use serde_json::json;

use crate::Ingredients;

pub(crate) fn config() -> &'static SessionConfig {
    static CONFIG: OnceLock<SessionConfig> = OnceLock::new();
    CONFIG.get_or_init(|| SessionConfig {
        model: Arc::from("kimi-k3"),
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
        late_system: &[],
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
