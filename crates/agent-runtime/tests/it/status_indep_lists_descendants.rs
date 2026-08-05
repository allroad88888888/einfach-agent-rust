//! 051 验收第一条：父 spawn 两个子，父调 `status` → 回来的 tool_result 列出两个
//! 后代及其 activity。
//!
//! 构造用的是 051 §注意点名的那个形状（阻塞 spawn 下最稳的那个）：**父在同一条
//! assistant 消息里发两个 spawn 加一个 status**——三个调用同批派发，两次 spawn 建出
//! 子 agent，status 当场读树。父的下一跳再调一次 status，于是同一棵树在两个时刻的
//! activity 都被断言到（`Idle` → `Done`）：这一条把「它读的是活状态」和「它读的是
//! 一句写死的话」区分开。
//!
//! 顺带守住那条边界：两段正文里都**没有**子 agent 的回答文本（那是 collect 的事，
//! 053）——虽然第二跳时那些文本就躺在同一条历史的隔壁块里。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::status_indep_support::{
    build_ctx, listed_activities, listed_ids, sse_text, sse_tool_call, sse_tool_calls, temp_dir,
    tool_result, wire_tool_name, Route, RoutedServer,
};

#[test]
fn a_parent_with_two_running_children_sees_both_of_them_and_their_activity() {
    let dir = temp_dir("status-lists");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);

    let server = RoutedServer::start(vec![
        // 越具体的 needle 越靠前：root 的第三跳请求体里同时有 call_r3 和 call_r4，
        // 反过来写的话它会被第二跳那条路由抢先命中，脚本就对不上了。
        Route {
            needle: "call_r4",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("all done, both branches reported"),
        },
        Route {
            needle: "call_r3",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_r4", &status_wire, "{}"),
        },
        Route {
            needle: "TASKALPHA",
            delay: Duration::from_millis(120),
            status: 200,
            lines: sse_text("answer alpha"),
        },
        Route {
            needle: "TASKBETA",
            delay: Duration::from_millis(120),
            status: 200,
            lines: sse_text("answer beta"),
        },
        Route {
            needle: "kickoff-status",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_r1",
                    &spawn_wire,
                    r#"{"task":"TASKALPHA read the alpha side"}"#,
                ),
                (
                    "call_r2",
                    &spawn_wire,
                    r#"{"task":"TASKBETA read the beta side"}"#,
                ),
                ("call_r3", &status_wire, "{}"),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-status split this in two and watch them",
    );
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();

    // --- 第一次 status：跟两个 spawn 同批，两个子刚被建出来 ---
    let (first, is_error) = tool_result(&session, &root, "call_r3");
    assert!(!is_error, "纯读不该失败：{first}");
    assert_eq!(
        listed_ids(&first),
        vec!["root/a1", "root/a2"],
        "该恰好列出两个后代：{first}"
    );
    assert_eq!(
        listed_activities(&first),
        vec!["Idle", "Idle"],
        "同批建出来、还没轮到它们跑：{first}"
    );
    // **这一刻它们还没有 task**，而且这不是 bug：任务文本是子 agent 的第一条 user
    // 消息，它由 spawn 截获产出、排在泵的待办队列里，要等下一次 `step` 才写进去
    // （dispatch.rs §「任务文本 = 子 agent 的第一条 user 消息」）。同批派发的 status
    // 撞的正是这个窗口。`task=(无)` 是如实报告，不是拿 id 顶替一个假的任务。
    // 窗口只有这一批那么宽——下面第二次 status 就有了。
    assert!(
        first.contains("task=(无)"),
        "同批新建的子还没有第一条 user 消息：{first}"
    );

    // --- 第二次 status：两个子都收工之后 ---
    let (second, is_error) = tool_result(&session, &root, "call_r4");
    assert!(!is_error, "{second}");
    assert_eq!(listed_ids(&second), vec!["root/a1", "root/a2"], "{second}");
    assert_eq!(
        listed_activities(&second),
        vec!["Done", "Done"],
        "同一个工具、同一棵树，activity 该跟着世界变——不变就说明它读的是一句写死的话：{second}"
    );
    assert!(
        second.contains("TASKALPHA") && second.contains("TASKBETA"),
        "子一旦真的开跑，每个后代该带上它自己的任务：{second}"
    );

    // --- 只暴露 activity + task，不暴露子的正文（ORCHESTRATION §三/五）---
    // 第二跳时 "answer alpha" 就在同一条历史的隔壁块里（spawn 那次调用的结果），
    // status 正文里仍然不许有它。
    for body in [&first, &second] {
        assert!(
            !body.contains("answer alpha"),
            "status 不该带上子 agent 的回答正文：{body}"
        );
        assert!(
            !body.contains("answer beta"),
            "status 不该带上子 agent 的回答正文：{body}"
        );
    }
    let spawn_result = tool_result(&session, &root, "call_r1").0;
    assert!(
        spawn_result.contains("answer alpha"),
        "正文走的是 spawn 那条路，这条得确实成立：{spawn_result}"
    );

    // --- 两个子真的是两个活 agent，不是渲染出来的字 ---
    let mut live = session.live_agents();
    live.sort();
    assert_eq!(live, vec![root.clone(), root.child(1), root.child(2)]);

    let root_text: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        root_text.iter().any(|t| t.contains("all done")),
        "父该正常收尾：{root_text:#?}"
    );
}
