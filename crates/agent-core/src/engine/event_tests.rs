//! [`Event`] 协议类型的单元测试。

use std::sync::Arc;

use crate::{
    Adjustment, AgentId, ContentBlock, Epoch, ErrorClass, Event, PrefixImage, StopReason,
    TokenUsage, ToolCallId,
};

fn all_variants() -> Vec<Event> {
    vec![
        Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("读一下 /tmp/a"),
            images: Vec::new(),
        },
        Event::ProviderDone {
            agent: AgentId::root(),
            epoch: Epoch(1),
            blocks: vec![ContentBlock::Text(Arc::from("好的"))],
            stop: StopReason::EndTurn,
            usage: TokenUsage {
                prompt: 100,
                completion: 20,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: Some(100),
            },
            adjustments: vec![Adjustment::ThinkingDisabledForToolChoice],
        },
        Event::ProviderFailed {
            agent: AgentId::root(),
            epoch: Epoch(1),
            class: ErrorClass::Retryable,
            message: Arc::from("429"),
        },
        Event::ToolResult {
            agent: AgentId::root(),
            epoch: Epoch(1),
            call_id: ToolCallId::new("call_1"),
            content: Arc::from("file contents"),
        },
        Event::ToolFailed {
            agent: AgentId::root(),
            epoch: Epoch(1),
            call_id: ToolCallId::new("call_2"),
            error: Arc::from("ENOENT"),
        },
        Event::Timeout {
            agent: AgentId::root(),
            epoch: Epoch(1),
            call_id: Some(ToolCallId::new("call_1")),
        },
        Event::Timeout {
            agent: AgentId::root(),
            epoch: Epoch(1),
            call_id: None,
        },
        Event::Cancel {
            agent: AgentId::root(),
        },
    ]
}

#[test]
fn roundtrip_all_variants() {
    let events = all_variants();
    let serialized = serde_json::to_string(&events).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<Event>>(&serialized).unwrap(),
        events
    );
}

/// 路由的输入端：每个变体都答得出「这是谁的事」。漏一个就是一条不知道该写谁槽位的
/// 事件（028）。
#[test]
fn agent_extractor_is_exhaustive() {
    for event in all_variants() {
        assert_eq!(event.agent(), &AgentId::root(), "{event:?}");
    }
}

/// 闸的输入端：每个变体要不要过闸，逐个断言。漏一个就是一条不校验 epoch 的回写路径
/// （红线 6）。
#[test]
fn epoch_extractor_is_exhaustive() {
    for event in all_variants() {
        let expected = match &event {
            Event::UserInput { .. } | Event::Cancel { .. } => None,
            _ => Some(Epoch(1)),
        };
        assert_eq!(event.epoch(), expected, "{event:?}");
    }
}

/// provider 超时和工具超时必须分得出——两者的转移不同。
#[test]
fn timeout_distinguishes_provider_from_tool() {
    let provider = Event::Timeout {
        agent: AgentId::root(),
        epoch: Epoch(1),
        call_id: None,
    };
    let tool = Event::Timeout {
        agent: AgentId::root(),
        epoch: Epoch(1),
        call_id: Some(ToolCallId::new("call_1")),
    };
    assert_ne!(provider, tool);
    assert_ne!(
        serde_json::to_string(&provider).unwrap(),
        serde_json::to_string(&tool).unwrap()
    );
}
