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

use super::{image_placeholder::dropped_image_placeholder, names};

/// 多段 system 拼成一条 system message 的正文。段间用空行分隔。
/// 全空（没有段、或段的文本都是空）时返回 `None`——不发空的 system 消息。
pub fn system_text(chunks: &[SystemChunk]) -> Option<String> {
    join_texts(chunks.iter().map(|c| &*c.text))
}

/// DeepSeek 的 `late_system` 落法：**base 段 + late 段拼进同一条 system 消息的正文**
/// （不是插新消息——038 实测插新 system 消息 120x 归零，改现有段尾保 ~91%）。
/// 拼法跟 [`system_text`] 一模一样，只是把 late 段接在 base 段后面。
pub fn system_text_folded(base: &[SystemChunk], late: &[SystemChunk]) -> Option<String> {
    join_texts(base.iter().chain(late).map(|c| &*c.text))
}

/// Kimi/GLM 的 `late_system` 落法：**一条独立的 `role:"system"` 消息**（消息级追加，
/// 038 实测 ~100% 保前缀、免费）。正文是 late 段合并后的文本；late 段全空 → `None`
/// （不发空消息）。调用方把它 push 到 history 末尾——对仅扩展匹配是一次严格延长。
pub fn late_system_message(late: &[SystemChunk]) -> Option<Value> {
    system_text(late).map(|text| json!({"role": "system", "content": text}))
}

/// 若干段文本用空行拼成一条，全空返回 `None`。[`system_text`] /
/// [`system_text_folded`] 共用——「怎么把多段 system 拼成一条正文」只有一处。
fn join_texts<'a>(texts: impl Iterator<Item = &'a str>) -> Option<String> {
    let text = texts
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    (!text.is_empty()).then_some(text)
}

/// 由调用方声明该请求是否支持图片的历史编码结果。
///
/// 共享 wire 层不认识 provider：图片能力是 adapter 传入的数据。不能编码的图片在
/// 这里降级为确定性文本并计数，调用方据此生成可见的降级信息。
#[derive(Debug, PartialEq)]
pub struct EncodedHistory {
    pub messages: Vec<Value>,
    pub dropped_images: usize,
}

/// 历史消息 → wire 消息数组（不含 system），按调用方给出的图片能力编码。
pub fn history_with_image_support(messages: &[Message], supports_images: bool) -> EncodedHistory {
    history(messages, supports_images, false)
}

/// DeepSeek thinking-mode tool continuation: replay reasoning only on the assistant
/// message that owns tool calls. Other messages and providers keep the default omission.
pub fn history_with_tool_reasoning(messages: &[Message], supports_images: bool) -> EncodedHistory {
    history(messages, supports_images, true)
}

fn history(messages: &[Message], supports_images: bool, tool_reasoning: bool) -> EncodedHistory {
    let mut out = Vec::with_capacity(messages.len());
    let mut dropped_images = 0;
    for msg in messages {
        dropped_images += push_message(msg, supports_images, tool_reasoning, &mut out);
    }
    EncodedHistory {
        messages: out,
        dropped_images,
    }
}

fn push_message(
    msg: &Message,
    supports_images: bool,
    tool_reasoning: bool,
    out: &mut Vec<Value>,
) -> usize {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut saw_reasoning = false;
    let mut content = Vec::new();
    let mut tool_calls = Vec::new();
    let mut results = Vec::new();
    let mut dropped_images = 0;
    let has_image = msg
        .blocks
        .iter()
        .any(|block| matches!(block, ContentBlock::Image { .. }));

    for block in &msg.blocks {
        match block {
            ContentBlock::Text(t) => {
                append_text(&mut text, t);
                if has_image && supports_images {
                    content.push(json!({"type": "text", "text": &**t}));
                }
            }
            ContentBlock::Thinking(value) => {
                saw_reasoning = true;
                reasoning.push_str(value);
            }
            ContentBlock::Image { reference, .. } if supports_images => content.push(json!({
                "type": "image_url",
                "image_url": {"url": &**reference},
            })),
            ContentBlock::Image {
                reference,
                name,
                mime,
            } => {
                append_text(
                    &mut text,
                    &dropped_image_placeholder(reference, name.as_deref(), mime),
                );
                dropped_images += 1;
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

    if !text.is_empty() || !content.is_empty() || !tool_calls.is_empty() {
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
            if has_image && supports_images {
                Value::Array(content)
            } else {
                json!(text)
            },
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
    dropped_images
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
        let history = history_with_image_support(
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
            false,
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
        let encoded = history_with_tool_reasoning(&messages, false).messages;
        assert_eq!(encoded[0]["reasoning_content"], json!("pull bytes"));
        assert_eq!(encoded[1]["reasoning_content"], json!("read bytes"));

        let default = history_with_image_support(&messages, false).messages;
        assert!(default[0].get("reasoning_content").is_none());
        assert!(default[1].get("reasoning_content").is_none());
    }

    /// 空消息不产出空壳——发出去只会让对方 400。
    #[test]
    fn empty_message_emits_nothing() {
        assert!(
            history_with_image_support(&[msg(Role::Assistant, vec![])], false)
                .messages
                .is_empty()
        );
    }
}
