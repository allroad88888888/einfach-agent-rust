//! 053 验收：**子失败 → collect 拿到 `is_error` 的 tool_result，loop 继续**。
//!
//! 跟 029 的 `spawn_indep_failure_propagation` 是同一条语义（`is_error` = 子
//! `Failed`，003 的哲学跨 agent 版：父看得到出了什么事，自己决定怎么办），只是
//! 这一次结果是**领**回来的而不是等回来的。两条路共用 `crate::child_outcome`
//! 那一份翻译，所以这里也顺带钉住「后台那条路没有偷偷换一套措辞」。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use crate::spawn_bg_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_results, wire_tool_name,
};

/// 让子先撞完 402 再让 root 醒来去领。
const ROOT_HOP2: Duration = Duration::from_millis(250);

#[test]
fn collecting_a_failed_background_child_yields_an_error_result_and_the_turn_goes_on() {
    let dir = temp_dir("collect-failure");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_cf",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("那件事没成，我换个办法"),
        },
        Route {
            needle: "call_bg_f",
            delay: ROOT_HOP2,
            status: 200,
            lines: sse_tool_call("call_cf", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route::http_error(
            "FAILTASK",
            402,
            r#"{"error":{"message":"payment required","code":"insufficient_balance"}}"#,
        ),
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_bg_f",
                &spawn_wire,
                r#"{"task":"FAILTASK 注定失败的一件","background":true}"#,
            ),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(
        &mut session,
        &mut ctx,
        "kickoff 开一个后台的，等会儿去领",
    ));

    // 003 跨 agent 版：一个子失败不中止父的 loop。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "父该照常收尾"
    );

    let child = AgentId::new("root/a1");
    assert!(
        matches!(session.status_of(&child), TurnStatus::Failed(_)),
        "子该落 Failed：{:?}",
        session.status_of(&child)
    );

    let results = tool_results(&session, &AgentId::root());
    assert_eq!(results.len(), 2, "spawn + collect 各一条：{results:#?}");
    assert_eq!(results[1].0, "call_cf");
    assert!(
        results[1].2,
        "领到一个失败的子，该是 is_error 的 tool_result：{results:#?}"
    );
    assert!(
        results[1].1.contains("子 agent 失败"),
        "措辞该跟阻塞 spawn 那条路一模一样（同一份 `child_outcome`）：{}",
        results[1].1,
    );

    // 而且模型确实看到了它并接着往下走：最后一跳的请求体里带着那条失败结果。
    let last = server
        .call("call_cf")
        .expect("root 该在拿到失败结果后接着发一跳");
    assert!(
        last.body.contains("call_cf"),
        "失败的 tool_result 该回填进下一跳：{}",
        last.body
    );
}
