//! 独立测试覆盖点 4：`undo` 连子树过真泵。
//!
//! spawn 一轮（root 委托给一个子，子回结果，root 汇总）→ `Session::undo_turn`
//! （CLI 的 `/undo` 就是这一行）→ 显式开新一轮 → 再问一句完全不相关的问题。
//! 断言：undo 之后活名单/消息清空；下一轮真的经 `run_turn`（走真泵，不是
//! 手工 `encode` 一次）发出去的请求体里，不含第一轮任何痕迹——子的 task
//! 文本、子的回答、root 的汇总文本，一个字都不该在。

mod spawn_indep_support;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use spawn_indep_support::{Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name};

#[test]
fn undo_after_a_spawn_turn_leaves_no_trace_in_the_next_real_request() {
    let dir = temp_dir("undo-subtree");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route { needle: "call_child", delay: Default::default(), status: 200, lines: sse_text("root summary SECRET_DELTA") },
        Route { needle: "CHILDTASK", delay: Default::default(), status: 200, lines: sse_text("child result SECRET_GAMMA") },
        Route {
            needle: "firstturn",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call("call_child", &spawn_wire, r#"{"task":"CHILDTASK handle SECRET_BETA"}"#),
        },
        Route { needle: "secondturn", delay: Default::default(), status: 200, lines: sse_text("second turn answer, nothing to do with the first") },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "firstturn please delegate to a helper SECRET_ALPHA");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(session.live_agents().len(), 2, "root + 一个子");
    assert!(!session.messages().is_empty());

    // --- /undo：028 已证的机制，这里过真 runner 链路 ---
    let report = session.undo_turn();
    assert!(matches!(report, agent_core::UndoReport::Applied { .. }), "{report:?}");
    assert_eq!(session.live_agents(), vec![AgentId::root()], "子该从活名单上消失");
    assert!(session.messages().is_empty(), "root 的消息也该清空");

    // --- 再问一轮：显式 begin_turn（`run_turn` 不替调用方决定新一轮从哪开始）。
    session.begin_turn();
    let status2 = run_turn(&mut session, &mut ctx, "secondturn totally unrelated question");
    assert_eq!(status2, TurnStatus::Done { truncated: false });

    let second_request = server.call("secondturn").expect("second turn must have gone out for real");
    for trace in [
        "firstturn",
        "SECRET_ALPHA",
        "CHILDTASK",
        "SECRET_BETA",
        "call_child",
        "SECRET_GAMMA",
        "SECRET_DELTA",
        "root summary",
    ] {
        assert!(!second_request.body.contains(trace), "第二轮请求体不该含第一轮的痕迹 {trace:?}: {}", second_request.body);
    }

    // 第二轮该干干净净地只有一条子请求：没有任何子被重新 spawn 出来。
    assert_eq!(session.live_agents(), vec![AgentId::root()], "第二轮结束后仍然只有 root");
}
