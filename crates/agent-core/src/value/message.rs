//! 消息与内容块：一次对话历史的最小可序列化单元。
//!
//! 这里只定型不定存储结构——历史用什么容器（`imbl::Vector` 之类）留给 022/009
//! 决定，这个模块只保证「一条消息长什么样」在整个仓库里是同一个类型。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{MessageId, ToolCallId};

/// 消息发出方。**最小集**：只有 `User` / `Assistant` 两种。
///
/// 工具执行结果不是第三个角色，而是 `Assistant` 消息里的一个
/// `ContentBlock::ToolResult`——多数 provider 的线协议会把它编码成
/// `role: "tool"` 或者 `role: "user"` 里的一个特殊块，但那是 adapter 层的编码
/// 细节（见 docs/ADAPTER.md），core 只认「谁在说话」这一个维度。
///
/// 109：`ts` feature 门后面导出——展开压缩摘要盖住的原始轮次要能在网络协议上
/// 认出说话人是谁（`GET /sessions/{id}/compaction_record`）。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Role {
    User,
    Assistant,
}

/// 一条消息里的一个内容块。一条消息可以有多个块——比如助手一轮里先 `Thinking`
/// 再 `Text`，再连续跟两个 `ToolUse`。
///
/// 大字段一律 `Arc` 包住（红线 5）：`store.get()` 每次读都要 clone 整条历史，
/// 文本 / JSON 越长这条越关键；`PartialEq` 因此也能走 `Arc` 的指针比较快路。
///
/// 109：`ts` feature 门后面导出——见 [`Role`] 同一条理由。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ContentBlock {
    /// 可见文本。
    Text(Arc<str>),
    /// 思维链文本。是否进最终 prompt 前缀、要不要在下一轮被砍掉，由 adapter
    /// 按能力位决定（docs/ADAPTER.md），core 只负责存下来。
    Thinking(Arc<str>),
    /// 模型发起的一次工具调用请求。`input` 用 `Arc<serde_json::Value>` 而不是
    /// 具体结构体：参数 schema 由工具自己定义，core 不解析参数含义，只透传。
    ToolUse {
        id: ToolCallId,
        name: Arc<str>,
        input: Arc<serde_json::Value>,
    },
    /// 一次工具调用的结果，靠 `id` 跟对应的 `ToolUse` 配对。**不是独立角色**——
    /// 见 `Role` 的文档注释。
    ToolResult {
        id: ToolCallId,
        content: Arc<str>,
        is_error: bool,
    },
}

/// 历史里的一条消息。
///
/// **只放完成的消息，流式中间态不进这里。** 模型边生成边吐 delta 时，累积状态
/// 活在别处（accumulator，见 docs/ADAPTER.md）；只有一轮生成完整结束、拿到最终
/// 的 block 列表，才铸成一个 `Message` 写进历史。这样保证两件事：一是历史里
/// 任意一条拿出来都是自洽的、可以直接喂回 provider；二是 undo/redo 的粒度是
/// 「一整条完成的消息」，不会停在半条消息上。
///
/// 109：`ts` feature 门后面导出——见 [`Role`] 同一条理由。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Message {
    pub id: MessageId,
    pub role: Role,
    pub blocks: Vec<ContentBlock>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// serde 往返：`to_string` → `from_str` → 值相等，覆盖每种 `ContentBlock` 变体。
    #[test]
    fn message_roundtrip() {
        let msg = Message {
            id: MessageId(1),
            role: Role::Assistant,
            blocks: vec![
                ContentBlock::Text(Arc::from("hello")),
                ContentBlock::Thinking(Arc::from("thinking...")),
                ContentBlock::ToolUse {
                    id: ToolCallId::new("call_1"),
                    name: Arc::from("fs/read"),
                    input: Arc::new(json!({"path": "/tmp/a"})),
                },
                ContentBlock::ToolResult {
                    id: ToolCallId::new("call_1"),
                    content: Arc::from("file contents"),
                    is_error: false,
                },
            ],
        };
        let s = serde_json::to_string(&msg).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn role_roundtrip() {
        for role in [Role::User, Role::Assistant] {
            let s = serde_json::to_string(&role).unwrap();
            assert_eq!(serde_json::from_str::<Role>(&s).unwrap(), role);
        }
    }
}
