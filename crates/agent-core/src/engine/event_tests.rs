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
        Event::CompactDone {
            agent: AgentId::root(),
            summary: Arc::from("前 12 条：用户要读一个文件，读到了。"),
            epoch: Epoch(1),
        },
        Event::CompactFailed {
            agent: AgentId::root(),
            epoch: Epoch(1),
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

/// 摘要回执**必须**过闸（红线 6）。上面那个穷举测试用的是 `_` 兜底，加变体时不会
/// 逼任何人回答；这一条把 105 的两个回执单独钉住：它们是
/// [`agent_core::Effect::Compact`] 的结果，摘要在飞期间用户 undo / 取消过，回来的
/// 正文盖住的范围就跟实际历史对不上。少了这个 `Some`，闸放行，107 的回写照写不误，
/// 而错的只是下一轮 prompt 里多一段少一段——不报错，人发现不了。
#[test]
fn compaction_results_are_epoch_gated() {
    let done = Event::CompactDone {
        agent: AgentId::root(),
        summary: Arc::from("摘要"),
        epoch: Epoch(7),
    };
    let failed = Event::CompactFailed {
        agent: AgentId::root(),
        epoch: Epoch(7),
    };
    assert_eq!(done.epoch(), Some(Epoch(7)));
    assert_eq!(failed.epoch(), Some(Epoch(7)));
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
