//! 独立测试覆盖点 3：`is_error` 传播语义。
//!
//! 两个子，一个 402（provider 报错，子落 `Failed`），一个正常收尾。断言父
//! 的第二跳请求体里两个 `tool_result` 都在（没有因为一个子失败就把另一个
//! 也弄丢）、失败那个在 wire 上可辨（025 的取舍：wire 上没有专门的
//! `is_error` 字段，靠错误文本本身；这里额外用 `ContentBlock::ToolResult::
//! is_error` 这个结构化字段做权威判定，wire 文本只做「确实带进去了」的佐证）、
//! 成功那个内容完整不受影响。

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::spawn_indep_support::{
    build_ctx, sse_text, sse_tool_calls, temp_dir, wire_tool_name, Route, RoutedServer,
};

#[test]
fn one_child_fails_with_402_the_other_succeeds_and_both_tool_results_reach_the_parent() {
    let dir = temp_dir("failure-propagation");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_ok",
            delay: Default::default(),
            status: 200,
            lines: sse_text("summary: one succeeded, one failed"),
        },
        Route {
            needle: "OKTASK",
            delay: Default::default(),
            status: 200,
            lines: sse_text("ok child finished successfully"),
        },
        Route::http_error(
            "FAILTASK",
            402,
            r#"{"error":{"message":"payment required","code":"insufficient_balance"}}"#,
        ),
        Route {
            needle: "kickoff2",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_ok",
                    &spawn_wire,
                    r#"{"task":"OKTASK do the good half"}"#,
                ),
                (
                    "call_fail",
                    &spawn_wire,
                    r#"{"task":"FAILTASK do the doomed half"}"#,
                ),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff2 split into a doomed half and a good half",
    );

    // 003 跨 agent 版：一个子失败不中止父的 loop。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "父该照常收尾，不因为一个子失败就整轮失败"
    );

    let root = AgentId::root();
    let ok_child = root.child(1);
    let fail_child = root.child(2);
    let mut live = session.live_agents();
    live.sort();
    let mut expected = vec![root.clone(), ok_child.clone(), fail_child.clone()];
    expected.sort();
    assert_eq!(
        live, expected,
        "两个子都该留在活名单上（失败的子不会被自动 despawn，029 的代价 1）"
    );

    assert_eq!(
        session.status_of(&ok_child),
        TurnStatus::Done { truncated: false }
    );
    assert!(
        matches!(session.status_of(&fail_child), TurnStatus::Failed(_)),
        "失败的子该落 Failed: {:?}",
        session.status_of(&fail_child)
    );

    // --- 结构化判定：is_error 字段本身 ---
    let tool_results: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                id,
                content,
                is_error,
            } => Some((id.clone(), content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_results.len(),
        2,
        "两个 tool_result 都该在: {tool_results:#?}"
    );

    let ok_result = tool_results
        .iter()
        .find(|(_, content, _)| content.contains("finished successfully"))
        .expect("ok 子的结果该在场");
    assert!(!ok_result.2, "成功的那个不该是 is_error: {ok_result:?}");

    let fail_result = tool_results
        .iter()
        .find(|(_, _, is_error)| *is_error)
        .expect("失败的那个该有一条 is_error 的 tool_result");
    assert_ne!(
        fail_result.1, ok_result.1,
        "失败与成功的 tool_result 内容不该雷同"
    );

    // --- wire 佐证：两条内容都真的进了父的第二跳请求体，且靠文本可辨 ---
    let hop2 = server
        .call("call_ok")
        .expect("root's second hop must have been called");
    assert!(
        hop2.body.contains(&*ok_result.1),
        "成功那条的内容该逐字出现在第二跳请求体里"
    );
    assert!(
        hop2.body.contains(&*fail_result.1),
        "失败那条的内容也该逐字出现在第二跳请求体里（没有因为失败就被吞掉）"
    );
    assert!(
        hop2.body.contains("call_ok") && hop2.body.contains("call_fail"),
        "两个 tool_call_id 都该在第二跳请求体里回填"
    );
}
