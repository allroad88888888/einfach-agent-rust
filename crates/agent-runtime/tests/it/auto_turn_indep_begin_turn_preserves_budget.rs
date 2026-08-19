//! 211 独立验收 · 第 3 条（内部调用路径版）：`run_auto_turns` 自己开一轮时会
//! 内部调 `begin_turn`（issue 211 §1：「预算减一 → begin_turn → drain_next_turn
//! → 继续跑」）——这条测的是**那次内部调用**没有顺手把预算重置回上限，不是
//! 测试自己另外手动调一次 `begin_turn`（那条更直接的检查已经跟在
//! `auto_turn_indep_chain_runs_and_stops.rs` 的断言尾巴上）。
//!
//! 手法：预算配成 2，只放一节链（root spawn 一个子、子留笔记、子收尾、root
//! 收尾），自驱动那一轮读到笔记之后**直接答完，不再留新笔记**——链自己没有
//! 继续往下长的动力，唯一能让第二轮跑起来的只有「begin_turn 顺手把预算重置回
//! 2」这个 bug。所以：**预算该停在 1，不是 2**，且只该发生一次自驱动、服务器
//! 只该被问过 5 次（1 节 4 跳 + 自驱动那次直接收尾 1 跳）。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_auto_turns;

use crate::auto_turn_indep_support::{
    Leg, RoutedServer, build_ctx, chain_routes_with_extra, terminal_route, temp_dir,
};

const KICKOFF: &str = "KICKOFF-begin-turn 只想跑一节";

#[test]
fn the_internal_begin_turn_call_inside_an_auto_turn_does_not_refill_the_budget() {
    let dir = temp_dir("auto-turn-begin-turn");

    let leg = Leg {
        trigger_needle: KICKOFF,
        spawn_call_id: "call_spawn_0bt",
        task_needle: "TASK-A1bt",
        send_call_id: "call_send_1bt",
        note_text: "BTNOTE-1",
        child_final_text: "A1-DONE-bt",
        root_final_text: "ROOT-T0-DONE-bt",
    };
    // 自驱动那一轮读到 `BTNOTE-1` 之后直接答完，不再 spawn——链到此为止，
    // 唯一还能让泵继续转的只剩「预算被谁悄悄续上了」这一种可能。
    let routes = chain_routes_with_extra(
        std::slice::from_ref(&leg),
        terminal_route("BTNOTE-1", "AUTOTURN-DONE-bt"),
    );
    let server = RoutedServer::start(routes);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    session.set_agent_limits(AgentLimits {
        max_auto_turns: 2,
        ..AgentLimits::default()
    });

    let status0 = agent_runtime::run_turn(&mut session, &mut ctx, KICKOFF)
        .expect("kickoff 不是 source failure");
    assert_eq!(status0, TurnStatus::Done { truncated: false });
    assert_eq!(session.auto_turn_budget(), 2, "真实用户输入把预算加满到 2");

    let statuses = run_auto_turns(&mut session, &mut ctx).expect("自驱动不是 source failure");

    assert_eq!(
        statuses,
        vec![TurnStatus::Done { truncated: false }],
        "笔记只够触发一轮自驱动——没有第二条笔记，也就没有第二轮"
    );
    assert_eq!(
        session.auto_turn_budget(),
        1,
        "这一轮内部调用的 begin_turn 不该把预算从 1 重置回配置的上限 2"
    );
    assert!(
        session.inbox_of(&AgentId::root()).is_empty(),
        "唯一一条笔记该被这一轮收走，收件箱该是空的"
    );
    assert_eq!(
        server.calls().len(),
        5,
        "1 节 4 跳 + 自驱动那次直接收尾 1 跳 = 5，多出来的调用就是预算被续上的证据：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );
}
