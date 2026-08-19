//! 212 验收 7：`await` 自己 / 不在会话里的 id / 不活的 id → `is_error`，
//! 这一轮继续跑完（不是卡住、不是整轮失败）。
//!
//! 三种坏目标一次说清（同一条 assistant 消息里三个并行调用）：
//!
//! - `await(自己)` —— `AwaitDenied::Yourself`；
//! - `await("root/zzz-nope")` —— 这个 id 从没在这个会话出现过，
//!   `AwaitDenied::NotInSession`；
//! - `await(一个已经被 despawn 掉的 id)` —— 出现过、现在不活，
//!   `AwaitDenied::NotLive`。这个死掉的 id 在 `run_turn` 之前直接用核心 API
//!   spawn 再 despawn 掉：从没有任何 `await` 读过它的 `Status`，所以这次
//!   despawn 不会撞上「还有外部读者」的闸（这条边界另有独立记录，见本文件
//!   底部注释）。
//!
//! 三次都当场拿到 `is_error`，A 收到之后正常回答、收敛，root 也正常收尾。
//!
//! 夹具复用 `await_indep_support`。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ChildConfig, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::await_indep_support::{
    AWAIT_WIRE, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name, Route, RoutedServer,
};

fn no_delay(needle: &'static str, lines: Vec<String>) -> Route {
    Route {
        needle,
        delay: Duration::ZERO,
        status: 200,
        lines,
    }
}

#[test]
fn awaiting_yourself_an_unknown_id_or_a_dead_id_all_settle_as_errors_and_the_turn_finishes() {
    let dir = temp_dir("await-invalid");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        no_delay("call_spawn_a_inv", sse_text("ROOTDONE-inv")),
        // A 第二跳：三次坏 await 都已经收敛（is_error）之后，A 照常回答。
        no_delay("call_a_self_inv", sse_text("ADONE-inv")),
        // A 第一跳：三个并行 await 调用。
        no_delay(
            "ATASK-inv",
            sse_tool_calls(&[
                ("call_a_self_inv", AWAIT_WIRE, r#"{"id":"root/a2"}"#),
                (
                    "call_a_missing_inv",
                    AWAIT_WIRE,
                    r#"{"id":"root/zzz-nope"}"#,
                ),
                ("call_a_dead_inv", AWAIT_WIRE, r#"{"id":"root/a1"}"#),
            ]),
        ),
        no_delay(
            "kickoff-inv",
            sse_tool_call(
                "call_spawn_a_inv",
                &spawn_wire,
                r#"{"task":"ATASK-inv"}"#,
            ),
        ),
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_await();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    // 死目标：spawn 再 despawn，从没被任何 await 读过，despawn 干净利落。
    let dead = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    assert_eq!(dead.as_str(), "root/a1");
    let _ = session.despawn_child(&dead).unwrap();
    assert!(!session.is_live(&dead));

    let status = run_turn(&mut session, &mut ctx, "kickoff-inv 派一个子去试三种坏目标")
        .expect("三种坏目标不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // A 是第二个孩子（第一个号已经被死掉的 root/a1 占掉，despawn 不回收号）。
    let a = AgentId::new("root/a2");
    assert_eq!(a.as_str(), "root/a2");

    let (self_content, self_is_error) = tool_result(&session, &a, "call_a_self_inv");
    assert!(self_is_error, "await 自己该被拒：{self_content}");

    let (missing_content, missing_is_error) = tool_result(&session, &a, "call_a_missing_inv");
    assert!(
        missing_is_error,
        "await 一个从没出现过的 id 该被拒：{missing_content}"
    );

    let (dead_content, dead_is_error) = tool_result(&session, &a, "call_a_dead_inv");
    assert!(dead_is_error, "await 一个已经死掉的 id 该被拒：{dead_content}");

    // 这一轮继续跑完，不是卡住、不是整轮失败。
    assert_eq!(session.status_of(&a), TurnStatus::Done { truncated: false });
    assert_eq!(session.status_of(&root), TurnStatus::Done { truncated: false });
}
