//! 026 等价重写自 `tool_convergence_all_failed.rs` /
//! `tool_convergence_duplicate_call_id.rs` / `tool_convergence_error_reaches_prompt.rs` /
//! `tool_convergence_scan_not_counter.rs`——003 收尾补测钉住的那四条边界。
//!
//! 跟 `session_tool_outcome.rs` 分文件的界线：那份测「一次回执落地之后会怎样」
//! （收敛时机、截断、协议违规），这份测「003 的三条验收各自的反例锚点」
//! （全败仍继续、重复不覆盖、错误文本进 prompt、收敛是扫不是计数）。

mod support;

use std::sync::Arc;

use agent_core::{ContentBlock, Effect, Notice, SlotState, ToolCallId, TurnStatus};

use support::session::{new_session, observe, session_with_pending_tools};

/// 003 验收 2：**全部**失败仍然继续（让模型看到全貌再决定），不是直接 `Failed`。
#[test]
fn all_tools_failing_still_converges_to_thinking_not_failed() {
    let mut s = session_with_pending_tools(&[
        ("call_1", "srv:fs/read"),
        ("call_2", "srv:fs/read"),
        ("call_3", "srv:fs/read"),
    ]);

    let _ = s.step(support::tool_failed_event(s.epoch(), "call_1", "boom 1"));
    let _ = s.step(support::tool_failed_event(s.epoch(), "call_2", "boom 2"));
    let effects = s.step(support::tool_failed_event(s.epoch(), "call_3", "boom 3"));

    assert_eq!(s.status(), TurnStatus::Thinking);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. }))
    );
    let msg = s.messages().back().unwrap().clone();
    assert_eq!(msg.blocks.len(), 3);
    assert!(
        msg.blocks
            .iter()
            .all(|b| matches!(b, ContentBlock::ToolResult { is_error: true, .. }))
    );
}

/// 唯一一个工具调用失败的最小复现，同一断言。
#[test]
fn a_single_tool_call_that_fails_alone_still_converges() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let effects = s.step(support::tool_failed_event(s.epoch(), "call_1", "boom"));

    assert_eq!(s.status(), TurnStatus::Thinking);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. }))
    );
}

/// 边角 1：**槽还在、但已经是 `Finished`** 那条分支（其余槽仍 Pending，尚未收敛），
/// 且第一次落地的内容必须原样保留到最终收敛拼进消息为止。
#[test]
fn second_result_for_an_already_finished_slot_does_not_overwrite() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "第一次"));
    let after_first = observe(&s);

    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "第二次"));
    assert_eq!(observe(&s), after_first, "重复回执不该改状态");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));

    // 换一种投递方式（ToolFailed）同样被拒——殊途同归也包括「拒绝重复」这件事。
    let effects = s.step(support::tool_failed_event(s.epoch(), "call_1", "第三次"));
    assert_eq!(observe(&s), after_first);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(Notice::ProtocolViolation { .. })]
    ));

    // 让 call_2 落地触发收敛：call_1 的内容必须是**第一次**的。
    let _ = s.step(support::tool_result_event(s.epoch(), "call_2", "ok 2"));
    let msg = s.messages().back().unwrap().clone();
    assert_eq!(
        msg.blocks[0],
        ContentBlock::ToolResult {
            id: ToolCallId::new("call_1"),
            content: Arc::from("第一次"),
            is_error: false
        }
    );
}

/// 003 验收 1 的最后一截：失败的**错误文本本身**逐字节原样进 prompt。
/// wire 那一侧 `is_error` 会被丢掉（025 的既定取舍），错误能不能进 prompt
/// 靠的只有 `content` 这一个字段。
#[test]
fn failed_tool_error_text_survives_verbatim_into_the_next_prompt_message() {
    let multiline = "Traceback:\n  line 1\n  line 2\n错误：无法打开 /tmp/x\t(EACCES)";
    let mut s = session_with_pending_tools(&[
        ("call_1", "srv:fs/read"),
        ("call_2", "srv:fs/read"),
        ("call_3", "srv:fs/read"),
    ]);

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok 1"));
    let _ = s.step(support::tool_failed_event(s.epoch(), "call_2", multiline));
    let _ = s.step(support::tool_result_event(s.epoch(), "call_3", "ok 3"));

    let msg = s.messages().back().unwrap().clone();
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = &msg.blocks[1]
    else {
        panic!("第二个块应该是 call_2 的结果");
    };
    assert!(*is_error);
    assert_eq!(&**content, multiline, "错误文本必须逐字节原样保留");
}

/// 003 验收 3 的原子图版：收敛判断**不是计数器**。
///
/// M1 的做法是手动把一个槽从 `Finished` 改回 `Pending`（直接给平结构字段赋值，
/// 模拟「undo 回滚了这个槽的回执」）。原子图版本没有那条后门，但有真家伙：
/// **`undo_step` 就是那次回滚本身**。这比 M1 那条更强——它证明的不只是「扫」，
/// 而是「回滚只写回 primitive，收敛这个答案是 derived 重算出来的」：计数器式实现
/// 在这条路径上必然对不上（槽位回来了、计数没回来），而且不报错。
#[test]
fn undoing_a_landed_result_flips_convergence_back_by_recomputation() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read"), ("call_2", "srv:fs/list")]);
    assert!(!s.tools_converged());

    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok 1"));
    assert!(!s.tools_converged(), "还有一个 Pending");
    let _ = s.step(support::tool_result_event(s.epoch(), "call_2", "ok 2"));
    assert!(s.tools_converged(), "两个槽都落地了 → 收敛（槽位已清空）");
    assert_eq!(s.status(), TurnStatus::Thinking);

    // 退回最后一条 entry：槽位回到「call_1 Finished + call_2 Pending」，
    // 收敛必须立刻翻回 false。
    let recomputes = s.debug_recompute_count();
    let _ = s.undo_step();
    assert!(
        s.debug_recompute_count() > recomputes,
        "答案必须是重算出来的，不是缓存住的"
    );
    let slots = s.tool_slots();
    assert_eq!(slots.len(), 2);
    assert!(matches!(slots[0].state, SlotState::Finished { .. }));
    assert!(matches!(slots[1].state, SlotState::Pending));
    assert!(!s.tools_converged(), "槽位回滚了，收敛必须跟着翻回来");
    assert_eq!(s.status(), TurnStatus::ToolsPending);

    // redo 再翻回去，两个方向都对。
    let _ = s.redo_step();
    assert!(s.tools_converged());
    assert_eq!(s.status(), TurnStatus::Thinking);
}

/// 零个槽位算收敛（没有东西要等）——`Idle` 的新会话就是这个形状。
#[test]
fn empty_slots_are_converged() {
    assert!(new_session().tools_converged());
}
