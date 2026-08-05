//! 验收 1：契约可序列化（红线 3 精神）。`Effect`、`Event`、`Notice` 每个变体都要
//! serde 往返；`Effect::ExecuteTool` 带一个真实的 `ToolCallRequest` 快照。

mod support;

use std::sync::Arc;

use agent_core::{
    Adjustment, ContentBlock, Effect, Epoch, ErrorClass, Event, Notice, StopReason, TokenUsage,
    TurnStatus,
};

fn roundtrip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).expect("序列化不应失败");
    let back: T = serde_json::from_str(&json).expect("反序列化不应失败");
    assert_eq!(&back, value, "序列化往返后应得到相等的值，json={json}");
}

// ---- Effect ----

#[test]
fn effect_call_provider_roundtrip() {
    roundtrip(&Effect::CallProvider {
        agent: support::agent(),
        epoch: Epoch::START,
    });
}

#[test]
fn effect_execute_tool_roundtrip_is_pure_data() {
    // 002 合并后的契约：只带名字+输入（core 没有工具表，不编造快照）。
    roundtrip(&Effect::ExecuteTool {
        agent: support::agent(),
        call_id: support::call_id(),
        tool: std::sync::Arc::from("srv:fs/read"),
        input: std::sync::Arc::new(serde_json::json!({"path": "/tmp/a.txt"})),
        epoch: Epoch::START,
    });
}

#[test]
fn effect_cancel_in_flight_roundtrip() {
    roundtrip(&Effect::CancelInFlight {
        epoch: Epoch::START,
    });
}

#[test]
fn effect_emit_roundtrip() {
    roundtrip(&Effect::Emit(Notice::TurnStatusChanged {
        status: TurnStatus::Thinking,
    }));
}

// ---- Notice ----

#[test]
fn notice_turn_status_changed_roundtrip_every_status() {
    for status in [
        TurnStatus::Idle,
        TurnStatus::Thinking,
        TurnStatus::ToolsPending,
        TurnStatus::Done { truncated: true },
        TurnStatus::Done { truncated: false },
        TurnStatus::Failed(agent_core::Failure::Cancelled),
        TurnStatus::Failed(agent_core::Failure::Provider(ErrorClass::Exhausted)),
    ] {
        roundtrip(&Notice::TurnStatusChanged { status });
    }
}

#[test]
fn notice_tool_output_truncated_roundtrip() {
    roundtrip(&Notice::ToolOutputTruncated {
        call_id: support::call_id(),
        original_bytes: 10_485_760,
        kept_bytes: 32_768,
    });
}

// ---- Event ----

#[test]
fn event_user_input_roundtrip() {
    roundtrip(&Event::UserInput {
        agent: support::agent(),
        text: Arc::from("hello"),
    });
}

#[test]
fn event_provider_done_roundtrip() {
    roundtrip(&Event::ProviderDone {
        agent: support::agent(),
        epoch: Epoch::START,
        blocks: vec![ContentBlock::Text(Arc::from("done"))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 10,
            completion: 5,
            cached: Some(0),
        },
        prefix: support::prefix_image(),
        adjustments: vec![Adjustment::ThinkingDisabledForToolChoice],
    });
}

#[test]
fn event_provider_failed_roundtrip() {
    roundtrip(&Event::ProviderFailed {
        agent: support::agent(),
        epoch: Epoch::START,
        class: ErrorClass::Retryable,
        message: Arc::from("rate limited"),
    });
}

#[test]
fn event_tool_result_roundtrip() {
    roundtrip(&Event::ToolResult {
        agent: support::agent(),
        epoch: Epoch::START,
        call_id: support::call_id(),
        content: Arc::from("file contents"),
    });
}

#[test]
fn event_tool_failed_roundtrip() {
    roundtrip(&Event::ToolFailed {
        agent: support::agent(),
        epoch: Epoch::START,
        call_id: support::call_id(),
        error: Arc::from("permission denied"),
    });
}

#[test]
fn event_timeout_roundtrip_both_call_id_forms() {
    roundtrip(&Event::Timeout {
        agent: support::agent(),
        epoch: Epoch::START,
        call_id: None,
    });
    roundtrip(&Event::Timeout {
        agent: support::agent(),
        epoch: Epoch::START,
        call_id: Some(support::call_id()),
    });
}

#[test]
fn event_cancel_roundtrip() {
    roundtrip(&Event::Cancel {
        agent: support::agent(),
    });
}
