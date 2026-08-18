//! 211 独立验收 · 第 6 条：**undo 不退还预算**。
//!
//! 自开一轮（花掉一格预算）→ `/undo`（`agent_runtime::undo::undo_turn`）→
//! 预算仍是减过的那个值，不是被这次撤销退还。issue 211 §4 原文：「钱已经烧
//! 掉了，退还等于交出一条『撤销 → 重跑 → 再撤销』的无限循环」。
//!
//! 顺带核对 undo 该做到的另一半：这一轮留下的笔记退回收件箱、这一轮的回应
//! 从历史里退掉——跟 `send_indep_next_turn.rs` 那条 `/undo` 测试同一个形状，
//! 唯一新增的是「预算不跟着退」。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use agent_core::{AgentId, AgentLimits, Deliver, Session, TurnStatus, UndoReport};
use agent_runtime::run_auto_turns;

use crate::auto_turn_indep_support::{
    Leg, RoutedServer, build_ctx, chain_routes_with_extra, index_of, terminal_route, temp_dir,
};

const KICKOFF: &str = "KICKOFF-undo 花一格就够";

#[test]
fn undoing_an_auto_turn_does_not_refund_the_budget_it_spent() {
    let dir = temp_dir("auto-turn-undo");

    let leg = Leg {
        trigger_needle: KICKOFF,
        spawn_call_id: "call_spawn_0undo",
        task_needle: "TASK-A1undo",
        send_call_id: "call_send_1undo",
        note_text: "UNDONOTE-1",
        child_final_text: "A1-DONE-undo",
        root_final_text: "ROOT-T0-DONE-undo",
    };
    let routes = chain_routes_with_extra(
        std::slice::from_ref(&leg),
        terminal_route("UNDONOTE-1", "AUTOTURN-DONE-undo"),
    );
    let server = RoutedServer::start(routes);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    session.set_agent_limits(AgentLimits {
        max_auto_turns: 2,
        ..AgentLimits::default()
    });

    let status0 = agent_runtime::run_turn(&mut session, &mut ctx, KICKOFF)
        .expect("kickoff 不是 source failure");
    assert_eq!(status0, TurnStatus::Done { truncated: false });
    let note_before_undo = session.inbox_of(&root);
    assert_eq!(note_before_undo.len(), 1);

    let statuses = run_auto_turns(&mut session, &mut ctx).expect("自驱动不是 source failure");
    assert_eq!(statuses, vec![TurnStatus::Done { truncated: false }]);
    assert_eq!(
        session.auto_turn_budget(),
        1,
        "自开这一轮该花掉一格：2 → 1"
    );
    assert!(session.inbox_of(&root).is_empty(), "笔记该被这一轮收走");
    assert!(index_of(&session, &root, "AUTOTURN-DONE-undo").is_some());

    // ---- 撤掉这一轮自开的 turn。----
    let report = agent_runtime::undo::undo_turn(&mut session, &mut ctx);
    match report {
        UndoReport::Applied { .. } => {}
        other => panic!("自开的一轮该能正常撤销：{other:?}"),
    }

    // ① 钱已经烧掉了，不退：预算仍是 1，不是回到 2。
    assert_eq!(
        session.auto_turn_budget(),
        1,
        "undo 不该把花掉的这一格预算退还——写成退还这条必红"
    );

    // ② 状态该退：笔记回到收件箱，这一轮的回应从历史里退掉。
    assert_eq!(
        session.inbox_of(&root),
        note_before_undo,
        "笔记该原样退回收件箱（from/text/when 逐字相同）"
    );
    assert_eq!(session.inbox_of(&root)[0].when, Deliver::NextTurn);
    assert!(
        index_of(&session, &root, "AUTOTURN-DONE-undo").is_none(),
        "自开那一轮的回应该被 undo 退掉"
    );
}
