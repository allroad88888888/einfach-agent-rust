//! 208 验收第 7 条：只回工具**条数**，不回名单——正文里不该出现任何工具全名
//! （比如 `srv:fs/read`）。工具表本来就在每一轮的 prompt 里，再列一遍是纯浪费，
//! 而且两份会不一致（issue 208 §注意）。
//!
//! `ToolTable::builtin()` 至少声明 `srv:fs/read`/`srv:fs/list`
//! （`tool_table_names.rs` 的 `reversibility_of` 表点名的既有工具），
//! 断言这些名字、以及 `self` 自己的全名都不出现在它自己的正文里。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_result, wire_tool_name,
};

#[test]
fn self_reports_a_count_never_a_roster_of_tool_names() {
    let dir = temp_dir("self-tool-count-only");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("done"),
        },
        Route {
            needle: "kickoff-tool-count",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_1", &self_wire, "{}"),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-tool-count 问一次自己")
        .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let (body, is_error) = tool_result(&session, &root, "call_1");
    assert!(!is_error, "纯读不该失败：{body}");

    for full_name in ["srv:fs/read", "srv:fs/list", agent_runtime::SELF_TOOL] {
        assert!(
            !body.contains(full_name),
            "self 的正文里不该出现任何工具全名（撞见了 {full_name}）：{body}"
        );
    }
}
