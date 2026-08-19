//! 212 验收 1：**兄弟互等**——A `await` 兄弟 B（B 是慢的）→ B 干完 → A 的槽收敛、
//! A 继续跑。这是「互相订阅」的行为证据：A 真的在 B 还没到终态时就已经挂起，
//! 不是碰巧问到一个已经收场的答案。
//!
//! 拓扑：root 一跳并行 foreground spawn 两个子——A（`root/a1`，立刻答一句
//! 「我要 await 你」的任务）与 B（`root/a2`，任务本身很慢，`Route::delay`
//! 撑住）。A 的第一跳直接调 `srv:agent/await`（缺省 `until`，即 `Settled`）
//! 指向 B；B 收敛之后 A 的槽自动跟着收敛，A 才发第二跳把自己的答案说完。
//!
//! 时序焊死：A 那一跳（零延迟）该在 B 那一跳（有延迟）结束之前就完成——
//! 这是「A 真的挂起了，不是事后才问」的硬证据；A 的第二跳（await 收敛之后
//! 那一跳）该晚于 B 收尾——这是「A 真的等到了」的硬证据。
//!
//! 夹具复用 `await_indep_support`（该模块顶部有黑盒来源声明）。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::await_indep_support::{
    AWAIT_WIRE, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name, Route, RoutedServer,
};

/// B 比 A 慢得多——大到不可能是巧合命中。
const SLOW: Duration = Duration::from_millis(400);

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

#[test]
fn a_waits_on_its_sibling_b_and_resumes_once_b_settles() {
    let dir = temp_dir("await-siblings");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // root 的第二跳：两个 spawn 槽都收敛之后，root 自己收尾。
        no_delay("call_spawn_a_sib", sse_text("ROOTDONE-sib")),
        // A 的第二跳：await 收敛之后，A 自己收尾。body 里带着 await 那次调用的
        // call_id——这就是「A 真的等到了才有这一跳」的凭据。
        no_delay("call_a_await_sib", sse_text("ADONE-sib")),
        // A 的第一跳：立刻请求 await(B)，缺省 until（Settled）。
        no_delay(
            "ATASK-sib",
            sse_tool_call(
                "call_a_await_sib",
                AWAIT_WIRE,
                r#"{"id":"root/a2"}"#,
            ),
        ),
        // B：故意很慢——它结束的时刻必须晚于 A 第一跳完成的时刻，这一条测试
        // 才测得出「挂起」而不是「事后一问就知道答案」。
        Route {
            needle: "BTASK-sib",
            delay: SLOW,
            status: 200,
            lines: sse_text("BDONE-sib"),
        },
        // root 的第一跳：并行 foreground spawn A 与 B。
        no_delay(
            "kickoff-sib",
            sse_tool_calls(&[
                ("call_spawn_a_sib", &spawn_wire, r#"{"task":"ATASK-sib"}"#),
                ("call_spawn_b_sib", &spawn_wire, r#"{"task":"BTASK-sib"}"#),
            ]),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_await();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-sib 派两个子，一个等另一个")
        .expect("兄弟互等不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a = AgentId::new("root/a1");
    let b = AgentId::new("root/a2");

    // ① 两个子都正常收场。
    assert_eq!(session.status_of(&a), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&b), TurnStatus::Done { truncated: false });

    // ② A 的 await 调用确实收敛成功（不是 is_error）。
    let (_content, is_error) = tool_result(&session, &a, "call_a_await_sib");
    assert!(!is_error, "B 正常收场，A 的 await 该成功收敛");

    // ③ 时序：A 第一跳该在 B 结束之前就完成——真的挂起了，不是巧合。
    let a_hop1 = server.call("ATASK-sib").expect("A 第一跳该被服务器记录");
    let b_call = server.call("BTASK-sib").expect("B 该被调用");
    assert!(
        a_hop1.end < b_call.end,
        "A 的第一跳该早于 B 结束就完成（A 已经挂起在等）：a_hop1.end={:?} b.end={:?}",
        a_hop1.end,
        b_call.end,
    );

    // ④ 时序：A 第二跳该在 B 结束之后才发出去——真的等到了才继续跑。
    let a_hop2 = server
        .call("call_a_await_sib")
        .expect("A 该在 await 收敛之后发第二跳");
    assert!(
        a_hop2.start > b_call.end,
        "A 该等到 B 落终态才恢复：a_hop2.start={:?} b.end={:?}",
        a_hop2.start,
        b_call.end,
    );

    // ⑤ root 也正常收场——一路继续跑完，不是卡住。
    assert_eq!(session.status_of(&root), TurnStatus::Done { truncated: false });
}
