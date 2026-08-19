//! 211 独立验收 · 第 8 条：**三处同一组数**。
//!
//! `AgentLimits { max_auto_turns: n }` 配下去之后：`session.auto_turn_budget()`
//! 该被一次真实用户输入加满到 n；`srv:agent/self` 回给模型看的正文里也该出现
//! 这个 n（决策 32 的既有规矩——启动参数配的数、真正拦人的数、模型看到的数，
//! 三处必须是同一组，不能有一处静默漏传）。
//!
//! `n` 选一个跟默认值（`DEFAULT_MAX_AUTO_TURNS = 3`）、跟 `max_depth`/
//! `max_children` 的默认值（3/8）都不同的数（这里用 5），才能把「读到了配置值」
//! 和「凑巧撞上别的数」区分开。
//!
//! 黑盒来源与「实现体没读」的声明见 `auto_turn_indep_support/mod.rs` 顶部。

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::auto_turn_indep_support::{
    RoutedServer, build_ctx, no_delay, sse_text, sse_tool_call, temp_dir, tool_result,
    wire_tool_name,
};

const KICKOFF: &str = "KICKOFF-selfcheck 问问自己还能自开几轮";
const N: u32 = 5;

#[test]
fn the_configured_max_auto_turns_shows_up_in_the_budget_and_in_self() {
    let dir = temp_dir("auto-turn-three-places");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_self_n", sse_text("SELFCHECK-DONE")),
        no_delay(KICKOFF, sse_tool_call("call_self_n", &self_wire, "{}")),
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    session.set_agent_limits(AgentLimits {
        max_auto_turns: N,
        ..AgentLimits::default()
    });

    let status = run_turn(&mut session, &mut ctx, KICKOFF).expect("kickoff 不是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // ① 配置 → `session.auto_turn_budget()`：真实用户输入把预算加满到 N。
    assert_eq!(
        session.auto_turn_budget(),
        N,
        "真实用户输入该把预算加满到配置的 max_auto_turns"
    );
    assert_eq!(session.agent_limits().max_auto_turns, N);

    // ② 配置 → `srv:agent/self` 的正文：模型看到的也该是 N，不是默认的 3。
    let (body, is_error) = tool_result(&session, &root, "call_self_n");
    assert!(!is_error, "纯读不该失败：{body}");
    let upper_needle = format!("上限 {N} 轮");
    assert!(
        body.contains(&upper_needle),
        "self 正文该说出配置的上限（{upper_needle}），不是写死的 3：{body}"
    );
    // 这一轮还没花过任何自驱动预算，「还能自己开几轮」也该恰好是 N。
    let left_needle = format!("还能自己往下开 {N} 轮");
    assert!(
        body.contains(&left_needle),
        "这一轮预算原封未动，「还剩」也该是 N：{body}"
    );
}
