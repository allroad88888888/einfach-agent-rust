//! 会话历史 → 页面能重画的一份 JSON。
//!
//! 刷新页面之后，历史是从 IndexedDB 的 journal **重放**出来的（决策 6：恢复就是
//! 从自己的 journal 忠实重放），不是页面自己存了一份 UI 状态。所以页面重画时唯一
//! 该问的人是 `Session`——这个文件就是那次询问的形状。
//!
//! `CreateSessionRequest` 没有、也不要「客户端灌历史」的入口（114 §范围第 4 条）：
//! 那会同时破坏红线 11 的前缀缓存和审计一致性。这里是**只读投影**，反方向不存在。

use agent_core::{AgentId, ContentBlock, Message, Role, Session};
use serde_json::{Value, json};

/// root agent 的全部消息，按顺序。
pub(crate) fn to_json(session: &Session) -> String {
    let messages: Vec<Value> = session
        .messages_of(&AgentId::root())
        .iter()
        .map(message_json)
        .collect();
    Value::Array(messages).to_string()
}

fn message_json(message: &Message) -> Value {
    json!({
        "role": match message.role {
            Role::User => "user",
            Role::Assistant => "assistant",
        },
        "blocks": message.blocks.iter().map(block_json).collect::<Vec<_>>(),
    })
}

fn block_json(block: &ContentBlock) -> Value {
    match block {
        ContentBlock::Text(text) => json!({ "kind": "text", "text": &**text }),
        ContentBlock::Thinking(text) => json!({ "kind": "thinking", "text": &**text }),
        ContentBlock::ToolUse { id, name, input } => json!({
            "kind": "tool_use",
            "id": &*id.0,
            "name": &**name,
            "input": &**input,
        }),
        ContentBlock::ToolResult {
            id,
            content,
            is_error,
        } => json!({
            "kind": "tool_result",
            "id": &*id.0,
            "content": &**content,
            "is_error": is_error,
        }),
        ContentBlock::Image {
            reference: _,
            mime,
            name,
        } => json!({
            // `reference` 是 provider 铸的、请求专属的字符串（`docs/IMAGES.md` §七）
            // ——不进这份给页面看的投影。
            "kind": "image",
            "mime": &**mime,
            "name": name.as_ref().map(|n| &**n),
        }),
    }
}
