//! 独立测试覆盖点 9：泵终态后无幽灵事件。
//!
//! 两个子并行完成、root 收尾之后，`run_turn` 已经返回——事件回调只会在
//! 调用 `run_turn` 的这个线程上、在它的调用栈内被喊到（`RunnerCtx` 的
//! 事件出口是同步回调，不是另开一条线程转发）。等 500ms，宿主的回调不该
//! 再收到任何新的 `AgentEvent`：既没有被放弃的 IO 线程尾巴直接绕过泵调用
//! 回调，也没有任何延迟触发的收尾通报。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::spawn_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_calls, temp_dir, wire_tool_name,
};

#[test]
fn after_the_pump_reaches_a_terminal_state_no_further_events_arrive() {
    let dir = temp_dir("no-ghost-events");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_p",
            delay: Default::default(),
            status: 200,
            lines: sse_text("both children done, wrapping up"),
        },
        Route {
            needle: "GHOSTP",
            delay: Default::default(),
            status: 200,
            lines: sse_text("child P done"),
        },
        Route {
            needle: "GHOSTQ",
            delay: Default::default(),
            status: 200,
            lines: sse_text("child Q done"),
        },
        Route {
            needle: "kickoff6",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_calls(&[
                ("call_p", &spawn_wire, r#"{"task":"GHOSTP first child"}"#),
                ("call_q", &spawn_wire, r#"{"task":"GHOSTQ second child"}"#),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(
        &mut session,
        &mut ctx,
        "kickoff6 spawn two children then wrap up",
    ));
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let events_at_return = events.borrow().len();
    assert!(
        events_at_return > 0,
        "这条测试要求真的发生过事件，不然「没有新事件」就是空话"
    );

    std::thread::sleep(Duration::from_millis(500));

    let events_after_wait = events.borrow().len();
    assert_eq!(
        events_after_wait, events_at_return,
        "run_turn 返回之后不该再有任何 AgentEvent 到达：return={events_at_return} after_wait={events_after_wait}"
    );

    // 服务器侧也该保持安静：没有任何被放弃的调用事后又发起新连接。
    assert_eq!(
        server.calls().len(),
        4,
        "该恰好四次调用：root 首跳 + 两个子 + root 第二跳，等待期间不该多出来"
    );
}
