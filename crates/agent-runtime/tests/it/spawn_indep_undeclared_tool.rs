//! 独立测试覆盖点 6：spawn 未声明即不存在。
//!
//! 宿主的工具表**没有**挂 `with_spawn`——`srv:agent/spawn` 这个名字压根没
//! 被声明。模型（脚本）硬发一次这个名字的调用，runner 的 spawn 截获闸
//! （`ToolTable::declares`）该判定它跟别的不存在的工具一样，走
//! `unknown_tool` 语义的路，**不长树**：`live_agents()` 只有 root。

use agent_core::{AgentId, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::spawn_indep_support::{
    build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name, Route, RoutedServer,
};

#[test]
fn a_host_without_spawn_declared_treats_it_as_an_unknown_tool_and_the_tree_does_not_grow() {
    let dir = temp_dir("undeclared-spawn");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_ghost",
            delay: Default::default(),
            status: 200,
            lines: sse_text("ok, no child was created"),
        },
        Route {
            needle: "kickoff3",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_ghost",
                &spawn_wire,
                r#"{"task":"should never create anything"}"#,
            ),
        },
    ]);

    // 关键：宿主的工具表没有 `.with_spawn(...)`——`srv:agent/spawn` 没被声明。
    let tools = agent_runtime::ToolTable::builtin();
    assert!(
        !tools.declares(agent_runtime::SPAWN_TOOL),
        "这条测试的前提就是宿主没声明 spawn"
    );

    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff3 try to spawn even though nobody declared it",
    )
    .expect("undeclared tool is represented in the turn status");

    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "003 哲学：工具失败不中止 loop，模型该能看到 is_error 自己收敛"
    );
    assert_eq!(
        session.live_agents(),
        vec![AgentId::root()],
        "没长出任何子——unknown_tool 不该凭空建出一棵树"
    );

    let tool_results: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_results.len(),
        1,
        "该有一条 tool_result，就是那个假冒的 spawn 调用: {tool_results:#?}"
    );
    assert!(
        tool_results[0].1,
        "宿主没声明的工具名该落 is_error（unknown_tool 语义）: {tool_results:#?}"
    );
}
