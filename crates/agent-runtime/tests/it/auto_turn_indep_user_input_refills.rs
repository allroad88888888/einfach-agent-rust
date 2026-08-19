//! 211 独立验收 · 第 4 条：**只有真实用户输入能把预算加满**。
//!
//! 手法：预算配成 1，一节链把它花光（自驱动那一轮读到笔记之后直接答完、不再
//! 留新笔记，budget 1→0）→ 断言预算见底 → 再喂一句**真实**用户输入 → 预算该
//! 回到配置的上限，且这句真实输入本身也照常被处理（进历史、有回应、轮次正常
//! 收尾）——不是「预算回来了但这句话被吞了」。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_auto_turns;

use crate::auto_turn_indep_support::{
    Leg, RoutedServer, build_ctx, calls_matching, chain_routes_with_extra, index_of,
    terminal_route, temp_dir,
};

const KICKOFF: &str = "KICKOFF-refill 先花光预算";
const SECOND_ASK: &str = "SECONDASK-refill 预算该回来了";

#[test]
fn only_a_real_user_turn_refills_the_budget_and_that_turn_is_handled_normally() {
    let dir = temp_dir("auto-turn-refill");

    let leg = Leg {
        trigger_needle: KICKOFF,
        spawn_call_id: "call_spawn_0refill",
        task_needle: "TASK-A1refill",
        send_call_id: "call_send_1refill",
        note_text: "REFILLNOTE-1",
        child_final_text: "A1-DONE-refill",
        root_final_text: "ROOT-T0-DONE-refill",
    };
    let mut routes = chain_routes_with_extra(
        std::slice::from_ref(&leg),
        terminal_route("REFILLNOTE-1", "AUTOTURN-DONE-refill"),
    );
    // 第二轮：真实用户输入之后，root 不需要 spawn 任何人，直接答完。这句话是
    // 全新的字面文本，不会跟链上任何一节撞车，插在最前面（最晚触发）是安全的。
    routes.insert(0, terminal_route(SECOND_ASK, "ROUND2DONE-refill"));
    let server = RoutedServer::start(routes);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_send();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    session.set_agent_limits(AgentLimits {
        max_auto_turns: 1,
        ..AgentLimits::default()
    });

    let status0 = agent_runtime::run_turn(&mut session, &mut ctx, KICKOFF)
        .expect("kickoff 不是 source failure");
    assert_eq!(status0, TurnStatus::Done { truncated: false });
    assert_eq!(session.auto_turn_budget(), 1);

    let statuses = run_auto_turns(&mut session, &mut ctx).expect("自驱动不是 source failure");
    assert_eq!(statuses, vec![TurnStatus::Done { truncated: false }]);

    // ---- 前提：预算真的见底了。----
    assert_eq!(
        session.auto_turn_budget(),
        0,
        "唯一一格预算该被这一轮花掉"
    );

    // ---- 喂一句真实用户输入。----
    session.begin_turn();
    agent_runtime::persist::sync(&mut ctx, &mut session);
    let status1 = agent_runtime::run_turn(&mut session, &mut ctx, SECOND_ASK)
        .expect("第二轮不是 source failure");
    assert_eq!(status1, TurnStatus::Done { truncated: false });

    // ① 预算回到配置的上限——不是继续停在 0，也不是被顺手加到别的数。
    assert_eq!(
        session.auto_turn_budget(),
        1,
        "真实用户输入该把预算加满回配置的上限（这里是 1）"
    );

    // ② 这句真实输入本身照常被处理：进了历史，也真的换来一次新的 provider 请求
    // 和回应——不是「预算被加满了，但这句话本身没人理」。
    assert!(
        index_of(&session, &root, SECOND_ASK).is_some(),
        "第二轮这句真实输入该进历史"
    );
    assert!(
        index_of(&session, &root, "ROUND2DONE-refill").is_some(),
        "第二轮该有正常的回应"
    );
    assert_eq!(
        calls_matching(&server, SECOND_ASK),
        1,
        "第二轮该恰好发生一次新的 provider 请求"
    );
}
