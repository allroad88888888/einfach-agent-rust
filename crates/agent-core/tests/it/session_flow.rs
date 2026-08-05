//! 026 等价重写自 005 的 harness 测试（`harness_happy_path.rs` /
//! `harness_tool_reorder.rs` / `harness_cancel_in_flight.rs`）里**属于 loop 语义**的
//! 那一半：完整无网络全流程、乱序回填仍按槽序、工具在飞时取消并挡掉迟到回执。
//!
//! **不重建 `Harness`**：`support/harness/` 那套 `MockProvider`/`MockExecutor`/驱动器
//! 是 005 交付的 mock 脚手架本身，它现在接在 `engine::step` 上，027 换接 runner 时
//! 跟着一起迁更合适（那时才知道 runner 要什么形状）。这里手工喂事件——本来 mock 就
//! 站在事件层，脚手架省下的只是循环，不是语义。

use crate::support;
use agent_core::{ContentBlock, Effect, Notice, ToolCallId, TurnStatus};

use crate::support::session::new_session;

/// 005 的核心验收：`UserInput` → `CallProvider` →（回 2 个 `ToolUse`）→
/// `ExecuteTool`×2 → 回填 → 再 `CallProvider` →（回 `EndTurn`）→ `Done`。
/// 全程零 sleep、零网络，最后断言消息形状与 `turns_used`。
#[test]
fn full_turn_with_two_parallel_tools_converges_to_done() {
    let mut s = new_session();

    // 1. 用户说话 → 第一次 CallProvider。
    let effects = s.step(support::user_input_event("读一下 a.txt 和 b.txt"));
    assert!(matches!(effects.last(), Some(Effect::CallProvider { .. })));
    assert_eq!(s.turns_used(), 1);
    assert_eq!(s.messages().len(), 1, "第一次请求时历史里只有用户那一条");

    // 2. 模型要调两个工具 → 两条 ExecuteTool。
    let effects = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")],
    ));
    let dispatched: Vec<ToolCallId> = effects
        .iter()
        .filter_map(|e| match e {
            Effect::ExecuteTool { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        dispatched,
        vec![ToolCallId::new("call_1"), ToolCallId::new("call_2")]
    );
    assert_eq!(s.status(), TurnStatus::ToolsPending);
    assert_eq!(s.messages().len(), 2);

    // 3. 两个结果回来（第二个先到）→ 收敛 → 第二次 CallProvider。
    let effects = s.step(support::tool_result_event(s.epoch(), "call_2", "b 的内容"));
    assert!(effects.is_empty(), "还有一个在飞");
    let effects = s.step(support::tool_result_event(s.epoch(), "call_1", "a 的内容"));
    assert!(matches!(effects.last(), Some(Effect::CallProvider { .. })));
    assert_eq!(s.turns_used(), 2);
    assert_eq!(
        s.messages().len(),
        3,
        "第二次请求时历史里是 user + tool_use + results"
    );

    // 4. 模型答完 → Done。
    let effects = s.step(support::provider_done_end_turn(
        s.epoch(),
        "两个文件都读完了",
    ));
    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    assert!(s.status().is_terminal());
    assert_eq!(
        effects,
        vec![Effect::Emit(Notice::TurnStatusChanged {
            status: TurnStatus::Done { truncated: false }
        })]
    );
    assert_eq!(s.messages().len(), 4);
    assert_eq!(s.turns_used(), 2, "整轮恰好两次 CallProvider");

    // 五次真的改了状态的转移 = 五条 entry（两条工具回执各占一条：第一条只写了
    // 那个槽、没收敛，照样是一次可回滚的状态变更），全部属于 turn 1。
    assert_eq!(s.history_len(), 5);
    assert_eq!(
        s.history()
            .entries()
            .map(|e| e.meta.label)
            .collect::<Vec<_>>(),
        vec![
            "user_input",
            "provider_done",
            "tool_result",
            "tool_result",
            "provider_done"
        ]
    );
    assert!(s.history().entries().all(|e| e.meta.turn_id == 1));
}

/// 005 / 003：乱序回填（第二个工具先回来）之后，拼进消息的顺序仍然是**槽序**
/// （模型请求顺序），不是到达顺序；其中一个失败也不中止。
#[test]
fn out_of_order_backfill_still_respects_slot_order_and_survives_a_failure() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("三个工具"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[
            ("call_1", "srv:fs/read"),
            ("call_2", "srv:fs/read"),
            ("call_3", "srv:fs/read"),
        ],
    ));

    // 到达顺序：3 → 2（失败）→ 1。
    let _ = s.step(support::tool_result_event(s.epoch(), "call_3", "三"));
    let _ = s.step(support::tool_failed_event(s.epoch(), "call_2", "二炸了"));
    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "一"));

    assert_eq!(s.status(), TurnStatus::Thinking, "部分失败不中止");
    let msg = s.messages().back().unwrap().clone();
    let ids: Vec<ToolCallId> = msg
        .blocks
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult { id, .. } => id.clone(),
            other => panic!("期待 ToolResult，收到 {other:?}"),
        })
        .collect();
    assert_eq!(
        ids,
        vec![
            ToolCallId::new("call_1"),
            ToolCallId::new("call_2"),
            ToolCallId::new("call_3")
        ],
        "槽序，不是到达序"
    );
}

/// 005：工具在飞时取消——epoch bump、槽全弃、`Failed(Cancelled)`；迟到的
/// `ToolResult`/`ToolFailed`（旧 epoch）被闸丢弃。这是 M1 那份「为 undo 的 epoch
/// 校验做准备」在 026 里的兑现：同一道闸，现在挡的是 undo 之后的幽灵结果。
#[test]
fn cancel_while_tools_in_flight_gates_the_late_results() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("跑两个工具"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read"), ("call_2", "srv:fs/read")],
    ));
    let in_flight = s.epoch();

    let effects = s.step(support::cancel_event());
    assert_eq!(effects[0], Effect::CancelInFlight { epoch: in_flight });
    assert_eq!(
        s.status(),
        TurnStatus::Failed(agent_core::Failure::Cancelled)
    );
    assert!(s.tool_slots().is_empty(), "槽全弃");
    assert_eq!(s.epoch(), in_flight.next());

    let history_len = s.history_len();
    for event in [
        support::tool_result_event(in_flight, "call_1", "回来晚了"),
        support::tool_failed_event(in_flight, "call_2", "也回来晚了"),
    ] {
        assert!(s.step(event).is_empty(), "迟到的回执被闸丢弃");
    }
    assert_eq!(s.history_len(), history_len, "被丢弃的回执不留任何痕迹");
}
