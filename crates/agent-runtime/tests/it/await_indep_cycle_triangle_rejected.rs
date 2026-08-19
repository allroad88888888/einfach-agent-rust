//! 212 验收 3：**三角环**——A→B、B→C、C→A，第三条被拒。两条边的直接互等
//! （上一份测试）不是唯一形状：只查「目标是不是直接在等我」会漏掉这种更长的环，
//! 必须真的顺着等待边走。
//!
//! 拓扑：root 一跳并行 foreground spawn A（`root/a1`）、B（`root/a2`）、
//! C（`root/a3`）三个子。三跳的延迟递增（A 零延迟、B 稍晚、C 更晚），保证
//! A→B、B→C 两条边确定性地先落地，C→A 那次请求落地时环已经在图上（走
//! A→B→C 正好回到 C 自己）。C 被拒之后正常收尾，链条从 C 往回收敛
//! （C 收敛满足 B 的等待、B 收敛满足 A 的等待），整轮不需要人工介入就正常结束。
//!
//! 夹具复用 `await_indep_support`。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::await_indep_support::{
    AWAIT_WIRE, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name, Route, RoutedServer,
};

const B_DELAY: Duration = Duration::from_millis(100);
const C_DELAY: Duration = Duration::from_millis(200);

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

#[test]
fn a_triangle_of_awaits_is_rejected_on_the_closing_edge() {
    let dir = temp_dir("await-cycle3");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // root 第二跳：三个 spawn 槽都收敛之后收尾。
        no_delay("call_spawn_a_c3", sse_text("ROOTDONE-c3")),
        // 三个子各自的第二跳，从里到外依次收敛。
        no_delay("call_a_await_c3", sse_text("ADONE-c3")),
        no_delay("call_b_await_c3", sse_text("BDONE-c3")),
        no_delay("call_c_await_c3", sse_text("CDONE-c3")),
        // A 第一跳：零延迟，await(B)。
        no_delay(
            "ATASK-c3",
            sse_tool_call("call_a_await_c3", AWAIT_WIRE, r#"{"id":"root/a2"}"#),
        ),
        // B 第一跳：稍晚，await(C)——落地时 A→B 已经在图上。
        Route {
            needle: "BTASK-c3",
            delay: B_DELAY,
            status: 200,
            lines: sse_tool_call("call_b_await_c3", AWAIT_WIRE, r#"{"id":"root/a3"}"#),
        },
        // C 第一跳：更晚，await(A)——落地时 A→B→C 已经在图上，这条边闭合成环。
        Route {
            needle: "CTASK-c3",
            delay: C_DELAY,
            status: 200,
            lines: sse_tool_call("call_c_await_c3", AWAIT_WIRE, r#"{"id":"root/a1"}"#),
        },
        // root 第一跳：并行 foreground spawn 三个子。
        no_delay(
            "kickoff-c3",
            sse_tool_calls(&[
                ("call_spawn_a_c3", &spawn_wire, r#"{"task":"ATASK-c3"}"#),
                ("call_spawn_b_c3", &spawn_wire, r#"{"task":"BTASK-c3"}"#),
                ("call_spawn_c_c3", &spawn_wire, r#"{"task":"CTASK-c3"}"#),
            ]),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_await();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-c3 三个子摆成一个环")
        .expect("三角环被拒不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let a = AgentId::new("root/a1");
    let b = AgentId::new("root/a2");
    let c = AgentId::new("root/a3");

    // A→B、B→C 两条边都成功建立。
    let (_a_content, a_is_error) = tool_result(&session, &a, "call_a_await_c3");
    assert!(!a_is_error, "A await B 该成功");
    let (_b_content, b_is_error) = tool_result(&session, &b, "call_b_await_c3");
    assert!(!b_is_error, "B await C 该成功");

    // C→A 这条闭合边被拒——本条验收的核心断言。
    let (c_content, c_is_error) = tool_result(&session, &c, "call_c_await_c3");
    assert!(c_is_error, "C await A 该被当场拒绝（三角环闭合）：{c_content}");
    // 拒绝文本至少点名 C 自己（发起方）和 A（目标）——链上不止两个 id。
    assert!(
        c_content.contains(c.as_str()),
        "拒绝文本该点名发起方 C：{c_content}"
    );
    assert!(
        c_content.contains(a.as_str()),
        "拒绝文本该点名目标 A：{c_content}"
    );

    // 全部正常收场——三个子和 root 都不卡住。
    assert_eq!(session.status_of(&a), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&b), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&c), TurnStatus::Done { truncated: false });
}
