//! core 的 `SystemChunk` / `Message` → OpenAI 兼容的 `messages` 数组，三家共用
//! （PROVIDERS.md：三家的流式/错误骨架都是「OpenAI 兼容」，请求侧的消息形状
//! 同样没有测出任何独立差异）。system 是 `messages` 里的第一条，工具结果是
//! `role: "tool"` 的独立消息。core 只认「谁在说话」两个角色，`ToolResult`
//! 是块不是角色——**把它编码成第三个 role 是本文件的活**（docs/ADAPTER.md）。
//!
//! 两处刻意的取舍，都写在这里而不是散在各家 `encode` 里：
//! - 默认不回传 `Thinking`。DeepSeek 的 thinking tool-call continuation 是唯一
//!   例外，由 adapter 显式选择 [`history_with_tool_reasoning`]；Kimi/GLM 仍走默认。
//! - **`ToolResult.is_error` 不进 wire**。OpenAI 系的 tool 消息没有这个字段，
//!   而往 content 里塞前缀等于改语义（adapter 明确不做的事）——错误信息本来
//!   就在 content 里。

use agent_core::{ContentBlock, Message, Role, SystemChunk};
use serde_json::{Map, Value, json};

use super::names;

/// 多段 system 拼成一条 system message 的正文。段间用空行分隔。
/// 全空（没有段、或段的文本都是空）时返回 `None`——不发空的 system 消息。
pub fn system_text(chunks: &[SystemChunk]) -> Option<String> {
    join_texts(chunks.iter().map(|c| &*c.text))
}

/// 若干段文本用空行拼成一条，全空返回 `None`。[`system_text`] 用它——
/// 「怎么把多段 system 拼成一条正文」只有一处。
fn join_texts<'a>(texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let text = texts
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// 历史消息 → wire 消息数组（不含 system）。
#[derive(Debug, PartialEq)]
pub struct EncodedHistory {
    pub messages: Vec<Value>,
}

/// 历史消息 → wire 消息数组（不含 system）。
pub fn history(messages: &[Message]) -> EncodedHistory {
    history_inner(messages, false)
}

/// DeepSeek thinking-mode tool continuation: replay reasoning only on the assistant
/// message that owns tool calls. Other messages and providers keep the default omission.
pub fn history_with_tool_reasoning(messages: &[Message]) -> EncodedHistory {
    history_inner(messages, true)
}

fn history_inner(messages: &[Message], tool_reasoning: bool) -> EncodedHistory {
    let mut out = Vec::with_capacity(messages.len());
    for msg in messages {
        push_message(msg, tool_reasoning, &mut out);
    }
    EncodedHistory { messages: out }
}

fn push_message(msg: &Message, tool_reasoning: bool, out: &mut Vec<Value>) {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut saw_reasoning = false;
    let mut tool_calls = Vec::new();
    let mut results = Vec::new();

    for block in &msg.blocks {
        match block {
            ContentBlock::Text(t) => {
                append_text(&mut text, t);
            }
            ContentBlock::Thinking(value) => {
                saw_reasoning = true;
                reasoning.push_str(value);
            }
            ContentBlock::ToolUse { id, name, input } => tool_calls.push(json!({
                "id": &*id.0,
                "type": "function",
                "function": {
                    "name": names::to_wire(name),
                    // 参数在 wire 上是**字符串**，不是对象。
                    "arguments": serde_json::to_string(&**input).unwrap_or_default(),
                }
            })),
            ContentBlock::ToolResult { id, content, .. } => results.push(json!({
                "role": "tool",
                "tool_call_id": &*id.0,
                "content": &**content,
            })),
        }
    }

    if !text.is_empty() || !tool_calls.is_empty() {
        let mut m = Map::new();
        m.insert(
            "role".into(),
            json!(match msg.role {
                Role::User => "user",
                Role::Assistant => "assistant",
            }),
        );
        m.insert(
            "content".into(),
            json!(text),
        );
        if !tool_calls.is_empty() {
            m.insert("tool_calls".into(), Value::Array(tool_calls));
            if tool_reasoning && saw_reasoning && msg.role == Role::Assistant {
                m.insert("reasoning_content".into(), json!(reasoning));
            }
        }
        out.push(Value::Object(m));
    }
    // 工具结果跟在发起它的那条消息之后——wire 上的配对靠 `tool_call_id`，
    // 但顺序错了有的实现会直接拒。
    out.extend(results);
}

fn append_text(text: &mut String, value: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{MessageId, ToolCallId};
    use std::sync::Arc;

    fn msg(role: Role, blocks: Vec<ContentBlock>) -> Message {
        Message {
            id: MessageId(1),
            role,
            blocks,
        }
    }

    #[test]
    fn system_chunks_join_with_blank_line() {
        let chunks = vec![
            SystemChunk {
                label: Arc::from("base"),
                text: Arc::from("你是助手"),
            },
            SystemChunk {
                label: Arc::from("skill"),
                text: Arc::from("会用工具"),
            },
        ];
        assert_eq!(system_text(&chunks).unwrap(), "你是助手\n\n会用工具");
        assert_eq!(system_text(&[]), None);
    }

    #[test]
    fn tool_use_and_result_become_wire_shapes() {
        let history = history(
            &[
                msg(Role::User, vec![ContentBlock::Text(Arc::from("北京天气"))]),
                msg(
                    Role::Assistant,
                    vec![
                        ContentBlock::Thinking(Arc::from("要调工具")),
                        ContentBlock::ToolUse {
                            id: ToolCallId::new("call_1"),
                            name: Arc::from("srv:fs/read"),
                            input: Arc::new(json!({"path": "/tmp/a"})),
                        },
                    ],
                ),
                msg(
                    Role::User,
                    vec![ContentBlock::ToolResult {
                        id: ToolCallId::new("call_1"),
                        content: Arc::from("晴"),
                        is_error: false,
                    }],
                ),
            ],
        )
        .messages;

        assert_eq!(history.len(), 3);
        assert_eq!(history[0], json!({"role": "user", "content": "北京天气"}));
        // 思考块不进 wire；工具名转义；arguments 是字符串。
        assert_eq!(
            history[1],
            json!({"role": "assistant", "content": "", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": {"name": "srv_3Afs_2Fread", "arguments": "{\"path\":\"/tmp/a\"}"}
            }]})
        );
        assert_eq!(
            history[2],
            json!({"role": "tool", "tool_call_id": "call_1", "content": "晴"})
        );
    }

    #[test]
    fn tool_reasoning_is_opt_in_and_kept_per_message() {
        let source = |id: &str, reasoning: &str| {
            msg(
                Role::Assistant,
                vec![
                    ContentBlock::Thinking(Arc::from(reasoning)),
                    ContentBlock::ToolUse {
                        id: ToolCallId::new(id),
                        name: Arc::from("web:source/read"),
                        input: Arc::new(json!({})),
                    },
                ],
            )
        };
        let messages = [
            source("call_1", "pull bytes"),
            source("call_2", "read bytes"),
        ];
        let encoded = history_with_tool_reasoning(&messages).messages;
        assert_eq!(encoded[0]["reasoning_content"], json!("pull bytes"));
        assert_eq!(encoded[1]["reasoning_content"], json!("read bytes"));

        let default = history(&messages).messages;
        assert!(default[0].get("reasoning_content").is_none());
        assert!(default[1].get("reasoning_content").is_none());
    }

    /// 空消息不产出空壳——发出去只会让对方 400。
    #[test]
    fn empty_message_emits_nothing() {
        assert!(
            history(&[msg(Role::Assistant, vec![])])
                .messages
                .is_empty()
        );
    }
}
