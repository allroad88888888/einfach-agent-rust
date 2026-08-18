//! 212 验收 2（本 issue 最硬的一条）：**环被挡在门口**——A `await` B 成功 →
//! B `await` A → B 那次当场拿到 `is_error`，两个 agent 都没卡住，这一轮
//! **正常结束**。断言拒绝文本里含 A 和 B 两个 id。
//!
//! 拓扑：root 一跳并行 foreground spawn A（`root/a1`）与 B（`root/a2`）。A 的
//! 第一跳零延迟请求 `await(B)`——先落地，边建成。B 的第一跳故意晚一点
//! （`Route::delay`）才请求 `await(A)`，保证它落地时 A→B 那条边已经在图上，
//! 这次请求因此确定性地撞上环，被拒；B 收到 `is_error` 之后正常收尾（不是
//! 卡住），B 收尾这件事本身又满足了 A 的等待条件（`Settled`），A 也跟着收敛。
//! 整轮因此**不需要任何超时或人工介入就能正常结束**——这正是「没卡住」的
//! 最直接证据。
//!
//! 夹具复用 `await_indep_support`。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::await_indep_support::{
    AWAIT_WIRE, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name, Route, RoutedServer,
};

/// B 的第一跳晚一点，保证服务器先把 A 那条零延迟的边落地。
const B_FIRST_HOP_DELAY: Duration = Duration::from_millis(150);

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

#[test]
fn b_awaiting_a_after_a_already_awaits_b_is_rejected_at_the_door() {
    let dir = temp_dir("await-cycle2");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // root 第二跳：两个 spawn 槽都收敛之后收尾。
        no_delay("call_spawn_a_c2", sse_text("ROOTDONE-c2")),
        // A 第二跳：它的 await(B) 在 B 收尾之后自动收敛，A 才说这句。
        no_delay("call_a_await_c2", sse_text("ADONE-c2")),
        // B 第二跳：await(A) 被拒之后，B 照常把这次拒绝当成一个普通的
        // tool_result 收下、正常回答——**没有卡住**。
        no_delay("call_b_await_c2", sse_text("BDONE-c2")),
        // A 第一跳：零延迟，立刻请求 await(B)。
        no_delay(
            "ATASK-c2",
            sse_tool_call("call_a_await_c2", AWAIT_WIRE, r#"{"id":"root/a2"}"#),
        ),
        // B 第一跳：晚一点才请求 await(A)——此时 A→B 那条边已经在图上，
        // 这次请求确定性地撞上环。
        Route {
            needle: "BTASK-c2",
            delay: B_FIRST_HOP_DELAY,
            status: 200,
            lines: sse_tool_call("call_b_await_c2", AWAIT_WIRE, r#"{"id":"root/a1"}"#),
        },
        // root 第一跳：并行 foreground spawn A 与 B。
        no_delay(
            "kickoff-c2",
            sse_tool_calls(&[
                ("call_spawn_a_c2", &spawn_wire, r#"{"task":"ATASK-c2"}"#),
                ("call_spawn_b_c2", &spawn_wire, r#"{"task":"BTASK-c2"}"#),
            ]),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_await();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff-c2 两个子互相 await")
        .expect("直接互等被拒不该是 source failure");

    // ① 这一轮正常结束——不是挂死、不是取消。
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a = AgentId::new("root/a1");
    let b = AgentId::new("root/a2");

    // ② A 的 await(B) 成功收敛（不是 is_error）——它先落地，没受影响。
    let (_a_content, a_is_error) = tool_result(&session, &a, "call_a_await_c2");
    assert!(!a_is_error, "A 先成功 await(B)，不该被后来的事波及");

    // ③ B 的 await(A) 当场拿到 is_error——这是本条最硬的断言。
    let (b_content, b_is_error) = tool_result(&session, &b, "call_b_await_c2");
    assert!(b_is_error, "B 后 await(A) 该被当场拒绝：{b_content}");

    // ④ 拒绝文本里含 A 和 B 两个 id——模型才知道是谁在等谁。
    assert!(
        b_content.contains(a.as_str()),
        "拒绝文本该点名 A（{}）：{b_content}",
        a.as_str()
    );
    assert!(
        b_content.contains(b.as_str()),
        "拒绝文本该点名 B（{}）：{b_content}",
        b.as_str()
    );

    // ⑤ 两个 agent 都没卡住：A、B、root 全部正常收场。
    assert_eq!(session.status_of(&a), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&b), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&root), TurnStatus::Done { truncated: false });
}
