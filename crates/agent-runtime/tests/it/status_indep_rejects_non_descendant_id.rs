//! 051 验收第二条：`status(id=<非后代>)` → `is_error` 的 tool_result，loop 继续、
//! 不 panic。
//!
//! 一次跑两种非法方向，它们是红线 10 的两条边：
//!
//! - **上读**：`root/a1` 问 `id=root`（自己的祖先）；
//! - **横读**：`root/a1` 问 `id=root/a2`（自己的兄弟，而且它此刻真的活着）。
//!
//! 两条都该是「回一句话给模型」而不是「掀桌」：被拒的那个 agent 照常收尾、父照常
//! 拿到结果、整轮照常落 `Done`（003 的哲学，跟 spawn 的提权拒绝一套规矩）。

mod status_indep_support;

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use status_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_calls, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn asking_about_an_ancestor_or_a_sibling_is_an_error_result_and_the_loop_keeps_going() {
    let dir = temp_dir("status-refusal");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);

    let server = RoutedServer::start(vec![
        Route { needle: "call_a2", delay: Duration::ZERO, status: 200, lines: sse_text("asked wrong, carried on anyway") },
        Route { needle: "call_r1", delay: Duration::ZERO, status: 200, lines: sse_text("all done") },
        Route {
            needle: "TASKPEEK",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_a1", &status_wire, r#"{"id":"root"}"#),
                ("call_a2", &status_wire, r#"{"id":"root/a2"}"#),
            ]),
        },
        Route { needle: "TASKOTHER", delay: Duration::from_millis(200), status: 200, lines: sse_text("other branch answer") },
        Route {
            needle: "kickoff-refusal",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_r1", &spawn_wire, r#"{"task":"TASKPEEK try to peek outside your own subtree"}"#),
                ("call_r2", &spawn_wire, r#"{"task":"TASKOTHER work the other branch"}"#),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default()).with_status();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-refusal one of them will try to peek");
    assert_eq!(status, TurnStatus::Done { truncated: false }, "被拒的调用不该把这一轮弄停");

    let root = AgentId::root();
    let peeker = root.child(1);

    // --- 上读：祖先 ---
    let (upward, is_error) = tool_result(&session, &peeker, "call_a1");
    assert!(is_error, "问自己的祖先该是 is_error：{upward}");
    assert!(upward.contains("root"), "拒绝文本该点名是哪个 id：{upward}");
    assert!(upward.contains("后代"), "拒绝文本该说清规则：{upward}");

    // --- 横读：活着的兄弟 ---
    let (sideways, is_error) = tool_result(&session, &peeker, "call_a2");
    assert!(is_error, "问自己的兄弟该是 is_error：{sideways}");
    assert!(sideways.contains("root/a2"), "拒绝文本该点名是哪个 id：{sideways}");
    // 被拒的原因是**方向**，不是「那个 id 不存在」——兄弟此刻确实在树上。
    assert!(session.live_agents().contains(&root.child(2)), "兄弟该真的活着，否则这条测的是另一件事");

    // --- loop 照常往下走 ---
    let peeker_text: Vec<_> = session
        .messages_of(&peeker)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert!(peeker_text.iter().any(|t| t.contains("carried on anyway")), "被拒的 agent 该照常收尾：{peeker_text:#?}");

    let root_text: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert!(root_text.iter().any(|t| t.contains("all done")), "父该照常拿到结果：{root_text:#?}");
}
