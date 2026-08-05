//! 026 等价重写自 `tool_outcome_convergence.rs`：`ToolsPending + ToolResult/ToolFailed`
//! 这一格「一次回执落地之后会怎样」——收敛时机（003）、截断（004/决策 19）、
//! 未知与重复 `call_id`（002 判断记录）、铸号单调。
//!
//! 003 收尾补测那四条边界（全败仍继续、重复不覆盖、错误文本进 prompt、收敛是扫不是
//! 计数）在 `session_tool_convergence.rs`。

mod support;

use std::sync::Arc;

use agent_core::{
    ContentBlock, DEFAULT_TOOL_OUTPUT_BYTES, Effect, Notice, Role, SlotState, ToolCallId,
    TurnStatus,
};

use support::session::{new_session, observe, session_with_pending_tools};

/// 两个槽：先落一个——没收敛，效果是空的（不是隐式忽略，是「等其余槽」）；再落最后
/// 一个——**这时候**才收敛：状态回 `Thinking`，两个结果按槽序（模型请求顺序）拼成
/// 一条消息，发 `TurnStatusChanged` + `CallProvider`。
#[test]
fn convergence_happens_only_when_the_last_slot_lands() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);
    let messages_before = s.messages().len();

    // 先落第二个槽——顺序倒着来，证明「收敛」看的是槽位状态而不是「第几个到」。
    let effects = s.step(support::tool_result_event(s.epoch(), "call_2", "list ok"));
    assert!(effects.is_empty(), "还有一个槽是 Pending，不该产出任何 effect");
    assert_eq!(s.status(), TurnStatus::ToolsPending);
    assert_eq!(s.messages().len(), messages_before, "未收敛之前不该动历史");
    assert!(!s.tools_converged());
    let slots = s.tool_slots();
    assert!(matches!(slots[0].state, SlotState::Pending));
    assert!(matches!(slots[1].state, SlotState::Finished { .. }));

    // 落最后一个槽——现在收敛。
    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "read ok"));
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert!(s.tool_slots().is_empty(), "收敛之后槽位应该清空，不留给下一轮");
    assert!(s.tools_converged(), "空槽位也算收敛（没有东西要等）");

    assert_eq!(effects.len(), 2);
    assert!(matches!(
        effects[0],
        Effect::Emit(Notice::TurnStatusChanged { status: TurnStatus::Thinking })
    ));
    assert!(matches!(effects[1], Effect::CallProvider { .. }));

    assert_eq!(s.messages().len(), messages_before + 1);
    let msg = s.messages().back().unwrap().clone();
    assert_eq!(msg.role, Role::Assistant);
    // 顺序是槽序（call_1、call_2），不是到达顺序（call_2 先到）。
    assert_eq!(
        msg.blocks,
        vec![
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_1"),
                content: Arc::from("read ok"),
                is_error: false
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_2"),
                content: Arc::from("list ok"),
                is_error: false
            },
        ]
    );
}

/// 003：3 个工具、1 个失败——loop 继续，不是直接 `Failed`，失败的那条
/// `is_error: true` 照样拼进下一轮的消息。
#[test]
fn partial_tool_failure_does_not_abort_the_turn() {
    let mut s = session_with_pending_tools(&[
        ("call_1", "srv:fs/read"),
        ("call_2", "srv:fs/read"),
        ("call_3", "srv:fs/read"),
    ]);

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok 1"));
    let _ = s.step(support::tool_failed_event(s.epoch(), "call_2", "boom"));
    let effects = s.step(support::tool_result_event(s.epoch(), "call_3", "ok 3"));

    assert_eq!(s.status(), TurnStatus::Thinking, "部分失败不中止，loop 继续");
    assert!(
        effects.iter().any(|e| matches!(e, Effect::CallProvider { .. })),
        "该接着调 provider"
    );

    let msg = s.messages().back().unwrap().clone();
    let ContentBlock::ToolResult { is_error, .. } = &msg.blocks[1] else {
        panic!("第二个块应该是 call_2 的结果");
    };
    assert!(*is_error, "失败的槽落地时 is_error 必须是 true");
    assert!(!matches!(&msg.blocks[0], ContentBlock::ToolResult { is_error: true, .. }));
    assert!(!matches!(&msg.blocks[2], ContentBlock::ToolResult { is_error: true, .. }));
}

/// 决策 19 / 004：超过 32KiB 的工具结果，落槽前被截断，报
/// `Notice::ToolOutputTruncated`，最终进消息历史的是**截断后**带标记的文本。
#[test]
fn oversized_tool_output_is_truncated_and_reported() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);

    let huge = "x".repeat(64 * 1024);
    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", &huge));

    let notices: Vec<_> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::Emit(Notice::ToolOutputTruncated { call_id, original_bytes, kept_bytes }) => {
                Some((call_id.clone(), *original_bytes, *kept_bytes))
            }
            _ => None,
        })
        .collect();
    assert_eq!(notices.len(), 1, "应该恰好报一条截断通报：{effects:?}");
    let (call_id, original_bytes, kept_bytes) = &notices[0];
    assert_eq!(*call_id, ToolCallId::new("call_1"));
    assert_eq!(*original_bytes, 64 * 1024);
    assert_eq!(*kept_bytes, DEFAULT_TOOL_OUTPUT_BYTES as u64);

    let msg = s.messages().back().unwrap().clone();
    let ContentBlock::ToolResult { content, .. } = &msg.blocks[0] else {
        panic!("期待 ToolResult 块");
    };
    assert!(content.len() < huge.len(), "消息里的内容应该比原始输出短");
    assert!(content.contains("输出被截断"), "必须带可见的截断标记");
    assert!(content.contains("原始 65536 字节"));
}

/// 未知 `call_id`（或者已经落过地的槽再来一次）：状态不变，报 `ProtocolViolation`
/// ——不是「等其余槽」，也不是 panic。
#[test]
fn unknown_or_duplicate_call_id_is_a_protocol_violation() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);

    let before = observe(&s);
    let effects = s.step(support::tool_result_event(s.epoch(), "call_unknown", "x"));
    assert_eq!(observe(&s), before, "未知 call_id 不该改状态，也不该留一步");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok"));
    let after_first = observe(&s);

    // 重复回执：槽已经不是 Pending 了（这里唯一的槽已经收敛、槽位被清空）。
    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "again"));
    assert_eq!(observe(&s), after_first, "重复回执不该改状态");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));
}

/// `MessageId` 铸号跨事件严格递增：user(1) → assistant/ToolUse(2) →
/// assistant/ToolResult(3) → assistant/EndTurn(4)。
#[test]
fn message_ids_stay_monotonic_across_a_whole_turn() {
    let mut s = new_session();

    let _ = s.step(support::user_input_event("读一下 a.txt"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "内容"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "读完了"));

    let ids: Vec<u64> = s.messages().iter().map(|m| m.id.0).collect();
    assert_eq!(ids, vec![1, 2, 3, 4]);
    assert_eq!(s.next_message_id().0, 5);
    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
}
