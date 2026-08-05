//! 026 等价重写自 `provider_done_stop_reason.rs`：`Thinking + ProviderDone` 的内部
//! 子分支——`StopReason` 五种取值各自的转移，加上「落历史无条件发生」这条。
//!
//! 断言逐条对应 M1 那份；`messages_before` 从 0 变成 1（fixture 是驱动出来的，
//! 用户那条消息已经在历史里），比较的仍然是**增量**。

mod support;

use std::sync::Arc;

use agent_core::{
    ContentBlock, Effect, ErrorClass, Failure, Notice, Role, SlotState, StopReason, ToolCallId,
    TurnStatus,
};

use support::session::thinking_session;

/// `EndTurn` → `Done { truncated: false }`；回复进历史；`prev_prefix.prompt_tokens`
/// 用这次的 `usage.prompt` 回填（纯赋值，不是判断）。
#[test]
fn end_turn_finishes_the_turn_and_backfills_prefix() {
    let mut s = thinking_session();
    let before = s.messages().len();

    let effects = s.step(support::provider_done_end_turn(s.epoch(), "好的"));

    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: false }
        })]
    );

    assert_eq!(s.messages().len(), before + 1, "assistant 回复应该进历史");
    assert_eq!(s.messages().back().unwrap().role, Role::Assistant);

    // 送进去的 `prefix.prompt_tokens` 是 `None`（fixture），回填后必须变成那次
    // usage 的 prompt——不是别的数、也不能还是 None。
    let prefix = s.prev_prefix().expect("prev_prefix 应该被存下来");
    assert_eq!(prefix.prompt_tokens, Some(42), "usage.prompt 在 fixture 里是 42");
}

/// `ToolUse` 且有 `ToolUse` 块：为每个块开一个 `Pending` 槽，顺序等于模型请求的
/// 顺序，状态转 `ToolsPending`，每个槽各发一个 `ExecuteTool`。
#[test]
fn tool_use_with_blocks_opens_slots_in_request_order() {
    let mut s = thinking_session();

    let effects = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")],
    ));

    assert_eq!(s.status(), TurnStatus::ToolsPending);
    let slots = s.tool_slots();
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0].call_id, ToolCallId::new("call_1"));
    assert_eq!(slots[1].call_id, ToolCallId::new("call_2"));
    assert!(slots.iter().all(|s| matches!(s.state, SlotState::Pending)));
    // 002 合并后的契约：槽只存名字+输入，不存编造的 location/reversibility。
    assert_eq!(&*slots[0].tool, "srv:fs/read");
    assert_eq!(&*slots[1].tool, "srv:fs/list");
    assert!(!s.tools_converged(), "两个槽都 Pending，derived 必须答未收敛");

    assert_eq!(effects.len(), 3, "1 条 TurnStatusChanged + 2 条 ExecuteTool");
    assert!(matches!(
        effects[0],
        Effect::Emit(Notice::TurnStatusChanged { status: TurnStatus::ToolsPending })
    ));
    let call_ids: Vec<_> = effects[1..]
        .iter()
        .map(|e| match e {
            Effect::ExecuteTool { call_id, .. } => call_id.clone(),
            other => panic!("期待 ExecuteTool，收到 {other:?}"),
        })
        .collect();
    assert_eq!(
        call_ids,
        vec![ToolCallId::new("call_1"), ToolCallId::new("call_2")]
    );
}

/// `ToolUse` 但没有任何 `ToolUse` 块：响应自相矛盾 → `ProtocolViolation`
/// （不是 `Failed`），`status` 留在 `Thinking` 不动，**但历史仍然无条件落地**。
#[test]
fn tool_use_claimed_without_blocks_is_a_protocol_violation() {
    let mut s = thinking_session();
    let before = s.messages().len();

    let effects = s.step(support::provider_done_tool_use_claimed_but_no_blocks(s.epoch()));

    assert_eq!(s.status(), TurnStatus::Thinking, "不知道该往哪走，status 不动");
    assert_eq!(s.messages().len(), before + 1, "历史仍然无条件落地");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));
    // 这一格**写了东西**（历史），所以跟 25 格「状态完全不变」的违规不同：
    // 它留下一条 entry，undo 退得掉那条被记下来的回复。
    assert_eq!(s.last_entry().unwrap().meta.label, "provider_done");
}

/// `MaxTokens` 不是停止条件，但响应确实被截断了——`Done { truncated: true }`
/// 如实标记，不会被误当成 `EndTurn`。
#[test]
fn max_tokens_finishes_the_turn_truncated() {
    let mut s = thinking_session();
    let effects = s.step(support::provider_done_with_stop(s.epoch(), StopReason::MaxTokens));

    assert_eq!(s.status(), TurnStatus::Done { truncated: true });
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: true }
        })]
    );
}

/// `StopSequence`：语义上是「答完了」，`truncated` 该是 `false`。
#[test]
fn stop_sequence_finishes_the_turn_not_truncated() {
    let mut s = thinking_session();
    let effects = s.step(support::provider_done_with_stop(s.epoch(), StopReason::StopSequence));

    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: false }
        })]
    );
}

/// 未知 `finish_reason` → `Failed(Provider(Unknown))`：不认识的 stop 当成功处理
/// 会静默吞掉一段可能被截断/出错的回复。
#[test]
fn unknown_stop_reason_fails_the_turn_as_unknown_provider_error() {
    let mut s = thinking_session();
    let effects = s.step(support::provider_done_with_stop(
        s.epoch(),
        StopReason::Other(Arc::from("weird")),
    ));

    let expected = TurnStatus::Failed(Failure::Provider(ErrorClass::Unknown));
    assert_eq!(s.status(), expected);
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged { status: expected })]
    );
}

/// 落历史无条件发生：连判 `ProtocolViolation` 的那一格也照落——上面那个测试已经
/// 数过条数，这里钉住**内容原样**（审计视角：即使响应自相矛盾，也要留下它原样
/// 说了什么）。
#[test]
fn the_reply_lands_in_history_verbatim_even_on_the_contradictory_branch() {
    let mut s = thinking_session();
    let _ = s.step(support::provider_done_tool_use_claimed_but_no_blocks(s.epoch()));

    let msg = s.messages().back().unwrap().clone();
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(
        msg.blocks,
        vec![ContentBlock::Text(Arc::from("我这就去调用工具"))]
    );
}
