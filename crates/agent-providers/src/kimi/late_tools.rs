//! Kimi 的消息级 late-tools 线协议。
//!
//! Kimi 可以在历史尾部追加一条没有 `content` 的 `role:system` 消息来携带中途
//! 激活的工具；这是这家独有的零缓存代价通道。

use agent_core::ToolSpec;
use serde_json::{Value, json};

use crate::wire::tools;

/// Kimi 没有公开工具数上限，不能凭空截断。
const MAX_TOOLS: usize = usize::MAX;

pub(super) fn message(late: &[ToolSpec]) -> Value {
    let value = tools::build(late, &[], MAX_TOOLS).value;
    json!({"role": "system", "tools": value})
}
