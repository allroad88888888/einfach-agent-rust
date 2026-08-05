//! 026 等价重写自 `timeout_transitions.rs`：`Event::Timeout` 的内部子分支（016）
//! ——provider 超时复用错误分流的重试路径，工具超时复用工具收敛的部分失败路径。

use crate::support;
use std::sync::Arc;

use agent_core::{ContentBlock, Effect, ErrorClass, Failure, Notice, ToolCallId, TurnStatus};

use crate::support::session::{observe, session_with_pending_tools, thinking_session};

/// provider 超时（`call_id: None`）跟 `ProviderFailed(Retryable)` 是同一条重试判断
/// 路径——预算耗尽之后同样落 `Failed(Provider(Retryable))`，不是单独一套「超时专属」
/// 的失败分类（`ErrorClass` 没有 `Timeout` 变体，016 的裁决就是复用 `Retryable`）。
#[test]
fn provider_timeout_exhausts_the_same_retry_budget_as_provider_failed() {
    let mut s = thinking_session();
    s.set_max_retries(1);

    let effects = s.step(support::timeout_event(s.epoch(), None));
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert_eq!(s.retries_used(), 1);
    assert!(matches!(effects[1], Effect::CallProvider { .. }));

    let effects = s.step(support::timeout_event(s.epoch(), None));
    let expected = TurnStatus::Failed(Failure::Provider(ErrorClass::Retryable));
    assert_eq!(s.status(), expected);
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged { status: expected })]
    );
}

/// 工具超时（`call_id: Some(_)`）：那个槽落 `Finished{is_error:true}`——超时也是
/// 一条结果（003 的部分失败语义），进消息历史的内容带着可见的超时文案。
#[test]
fn tool_timeout_finishes_the_slot_as_an_error_result() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);

    let effects = s.step(support::timeout_event(
        s.epoch(),
        Some(ToolCallId::new("call_1")),
    ));

    assert_eq!(s.status(), TurnStatus::Thinking, "唯一的槽落地就收敛了");
    assert!(s.tool_slots().is_empty());
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. }))
    );

    let msg = s.messages().back().expect("收敛应该拼出一条消息").clone();
    let ContentBlock::ToolResult {
        is_error, content, ..
    } = &msg.blocks[0]
    else {
        panic!("期待 ToolResult 块");
    };
    assert!(*is_error, "超时必须标 is_error:true");
    assert!(!content.is_empty(), "超时文案不能是空字符串");
}

/// 003：多个工具槽，其中一个超时、其余正常返回——不中止，超时的那一个照样拼进
/// 消息、`is_error:true`，跟 `ToolFailed` 部分失败同构。
#[test]
fn tool_timeout_among_multiple_slots_does_not_abort_the_turn() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok"));
    let effects = s.step(support::timeout_event(
        s.epoch(),
        Some(ToolCallId::new("call_2")),
    ));

    assert_eq!(
        s.status(),
        TurnStatus::Thinking,
        "部分超时不中止，loop 继续"
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. }))
    );

    let msg = s.messages().back().unwrap().clone();
    let ContentBlock::ToolResult { is_error: err1, .. } = &msg.blocks[0] else {
        panic!("call_1")
    };
    let ContentBlock::ToolResult { is_error: err2, .. } = &msg.blocks[1] else {
        panic!("call_2")
    };
    assert!(!err1, "call_1 正常返回");
    assert!(err2, "call_2 超时");
}

/// 工具超时事件带着一个不存在的 `call_id`：走的是 `on_tool_outcome` 本来就有的
/// 「未知/重复 call_id」判断——`ProtocolViolation`，不是新开一条判断路径。
#[test]
fn tool_timeout_with_unknown_call_id_is_a_protocol_violation() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let before = observe(&s);

    let effects = s.step(support::timeout_event(
        s.epoch(),
        Some(ToolCallId::new("call_unknown")),
    ));

    assert_eq!(observe(&s), before);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));
}

/// 超时文案必须逐字节确定（红线 11 的精神：它会进 `ContentBlock::ToolResult`
/// 从而进消息历史，历史最终会被送进下一轮请求，不能带时间戳或者等了多久）。
#[test]
fn tool_timeout_message_is_deterministic() {
    let content = |label: &str| -> Arc<str> {
        let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
        let _ = s.step(support::timeout_event(
            s.epoch(),
            Some(ToolCallId::new("call_1")),
        ));
        match &s.messages().back().unwrap().blocks[0] {
            ContentBlock::ToolResult { content, .. } => content.clone(),
            other => panic!("{label}：期待 ToolResult，收到 {other:?}"),
        }
    };
    assert_eq!(content("a"), content("b"));
}
