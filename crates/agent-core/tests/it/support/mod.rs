//! 测试共用的样例数据构造函数。**只负责「造一份确定的输入」**，不含任何断言。
//!
//! 不同测试文件各用其中一部分，未使用的部分会被 dead_code 警告到——整个模块
//! 允许 dead_code：它是给多个独立测试二进制共享的素材篮子，不是每篮都被每个
//! 文件取用。

#![allow(dead_code)]

use std::sync::Arc;

/// 026 的 `Session` fixture——见 `support/session.rs` 顶部文档。独立子模块：
/// 会话装配跟下面这批「造一份确定的输入」的事件/值构造函数不该混在一处。
///
/// 005 的 mock 脚手架（`MockProvider`/`MockExecutor`/`Harness`）连同它唯一的
/// 消费者（M1 的 `harness_*.rs` 系列）在 027 一起退役：那套脚手架接在
/// `engine::step`/`TurnState` 上，027 把 runner/CLI 换接到 `Session` 之后，
/// 它测的场景已经被 `session_flow.rs`（用真 `Session` + 真转移表）取代——
/// 026 的实做记录早就预告了这一步（「随 runner 一起迁」），这里选的是「迁移
/// 到已经存在的等价测试」而不是重新造一套 `Session` 版脚手架。
pub mod session;

use agent_core::{
    AgentId, ContentBlock, Epoch, ErrorClass, Event, Location, Message, MessageId, PrefixImage,
    Reversibility, Role, Segment, SegmentImage, StopReason, TokenUsage, ToolCallId,
    ToolCallRequest,
};

/// M1 唯一存在的 agent。
pub fn agent() -> AgentId {
    AgentId::root()
}

/// 一次真实的工具调用请求快照（`value/tool.rs` 的类型，M2 command 层记录用；
/// 002 合并后 `Effect::ExecuteTool` 不再携带它——core 没有工具表，只带名字+输入）。
pub fn tool_call_request() -> ToolCallRequest {
    ToolCallRequest {
        tool: Arc::from("srv:fs/read"),
        input: Arc::new(serde_json::json!({"path": "/tmp/a.txt"})),
        location: Location::Server,
        reversibility: Reversibility::Pure,
    }
}

pub fn call_id() -> ToolCallId {
    ToolCallId::new("call_1")
}

pub fn prefix_image() -> PrefixImage {
    PrefixImage {
        segments: vec![SegmentImage {
            segment: Segment::Tools,
            bytes: 128,
            hash: 7,
        }],
        prompt_tokens: Some(100),
    }
}

pub fn assistant_message() -> Message {
    Message {
        id: MessageId(1),
        role: Role::Assistant,
        blocks: vec![ContentBlock::Text(Arc::from("hi"))],
    }
}

// 002 转移表测试专用的事件构造函数。只负责「造一份确定的输入」，跟文件顶部
// 那批一样不含断言；单独一段是因为 002 之前没有 `step` 的实现，这批事件都要
// 真的喂给它。

pub fn user_input_event(text: &str) -> Event {
    user_input_for(&agent(), text)
}

/// 028：同一件事，但**指名道姓给哪个 agent**。事件的 `agent` 字段从 028 起真正
/// 路由（`Session::step`），多 agent 的测试全部经这一批构造函数。
pub fn user_input_for(agent: &AgentId, text: &str) -> Event {
    Event::UserInput {
        agent: agent.clone(),
        text: Arc::from(text),
        images: Vec::new(),
    }
}

/// `stop = EndTurn` 的 `ProviderDone`：纯文本回复，没有工具调用。
pub fn provider_done_end_turn(epoch: Epoch, text: &str) -> Event {
    provider_done_end_turn_for(&agent(), epoch, text)
}

pub fn provider_done_end_turn_for(agent: &AgentId, epoch: Epoch, text: &str) -> Event {
    Event::ProviderDone {
        agent: agent.clone(),
        epoch,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 42,
            completion: 7,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    }
}

/// `stop = ToolUse` 的 `ProviderDone`，按给定的 `(call_id, tool_name)` 列表造
/// 对应的 `ToolUse` 块，顺序就是传入的顺序（对应「模型请求的顺序」）。
pub fn provider_done_tool_use(epoch: Epoch, calls: &[(&str, &str)]) -> Event {
    provider_done_tool_use_for(&agent(), epoch, calls)
}

pub fn provider_done_tool_use_for(agent: &AgentId, epoch: Epoch, calls: &[(&str, &str)]) -> Event {
    let blocks = calls
        .iter()
        .map(|(id, name)| ContentBlock::ToolUse {
            id: ToolCallId::new(*id),
            name: Arc::from(*name),
            input: Arc::new(serde_json::json!({})),
        })
        .collect();
    Event::ProviderDone {
        agent: agent.clone(),
        epoch,
        blocks,
        stop: StopReason::ToolUse,
        usage: TokenUsage {
            prompt: 42,
            completion: 7,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    }
}

/// `stop = ToolUse` 但 `blocks` 里没有任何 `ToolUse` 块——002 判断记录里那个
/// 「响应自相矛盾」的边界情况。
pub fn provider_done_tool_use_claimed_but_no_blocks(epoch: Epoch) -> Event {
    Event::ProviderDone {
        agent: agent(),
        epoch,
        blocks: vec![ContentBlock::Text(Arc::from("我这就去调用工具"))],
        stop: StopReason::ToolUse,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    }
}

pub fn provider_done_with_stop(epoch: Epoch, stop: StopReason) -> Event {
    Event::ProviderDone {
        agent: agent(),
        epoch,
        blocks: Vec::new(),
        stop,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    }
}

pub fn tool_result_event(epoch: Epoch, id: &str, content: &str) -> Event {
    tool_result_for(&agent(), epoch, id, content)
}

pub fn tool_result_for(agent: &AgentId, epoch: Epoch, id: &str, content: &str) -> Event {
    Event::ToolResult {
        agent: agent.clone(),
        epoch,
        call_id: ToolCallId::new(id),
        content: Arc::from(content),
    }
}

pub fn tool_failed_event(epoch: Epoch, id: &str, error: &str) -> Event {
    Event::ToolFailed {
        agent: agent(),
        epoch,
        call_id: ToolCallId::new(id),
        error: Arc::from(error),
    }
}

pub fn cancel_event() -> Event {
    Event::Cancel { agent: agent() }
}

pub fn timeout_event(epoch: Epoch, call_id: Option<ToolCallId>) -> Event {
    Event::Timeout {
        agent: agent(),
        epoch,
        call_id,
    }
}

pub fn provider_failed_event(epoch: Epoch) -> Event {
    provider_failed_event_with_class(epoch, ErrorClass::Retryable)
}

/// 跟 [`provider_failed_event`] 一样，但 `class` 可选——016 的错误分流测试要
/// 覆盖 `Retryable` 之外的四个变体（`BadRequest`/`Auth`/`Exhausted`/`Unknown`）。
pub fn provider_failed_event_with_class(epoch: Epoch, class: ErrorClass) -> Event {
    Event::ProviderFailed {
        agent: agent(),
        epoch,
        class,
        message: Arc::from("boom"),
    }
}

/// 一个待定的工具槽——`ToolsPending` 状态下测试用。
pub fn pending_slot(id: &str) -> agent_core::ToolSlot {
    agent_core::ToolSlot {
        call_id: ToolCallId::new(id),
        tool: std::sync::Arc::from("srv:fs/read"),
        input: std::sync::Arc::new(serde_json::json!({"path": "/tmp/a.txt"})),
        state: agent_core::SlotState::Pending,
    }
}
