//! 051 验收第二条（**207 按决策 35 改写**）：`status(id=<不在活树上>)` →
//! `is_error` 的 tool_result，loop 继续、不 panic。
//!
//! **这个文件原本测的是别的东西。** 红线 10 改写之前它一次跑两种「非法方向」——
//! 上读祖先、横读兄弟——断言两条都被拒。横读全开之后那两条**都是合法的**，
//! 于是同一套脚手架现在同时钉两件事：
//!
//! - **兄弟的 id 现在通得过**（`root/a1` 问 `id=root/a2`，而且它此刻真的活着）；
//! - **只剩「不在活树上」一种拒绝**（`root/a1` 问 `id=root/a9`，从没 spawn 过）。
//!
//! 被拒的那条仍然该是「回一句话给模型」而不是「掀桌」：被拒的 agent 照常收尾、
//! 父照常拿到结果、整轮照常落 `Done`（003 的哲学，跟 spawn 的提权拒绝一套规矩）。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::status_indep_support::{
    build_ctx, sse_text, sse_tool_calls, temp_dir, tool_result, wire_tool_name, Route, RoutedServer,
};

#[test]
fn an_absent_id_is_an_error_result_while_a_live_sibling_now_resolves() {
    let dir = temp_dir("status-refusal");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_a2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("asked wrong, carried on anyway"),
        },
        Route {
            needle: "call_r1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("all done"),
        },
        Route {
            needle: "TASKPEEK",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_a1", &status_wire, r#"{"id":"root/a9"}"#),
                ("call_a2", &status_wire, r#"{"id":"root/a2"}"#),
            ]),
        },
        Route {
            needle: "TASKOTHER",
            delay: Duration::from_millis(200),
            status: 200,
            lines: sse_text("other branch answer"),
        },
        Route {
            needle: "kickoff-refusal",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_r1",
                    &spawn_wire,
                    r#"{"task":"TASKPEEK try to peek outside your own subtree"}"#,
                ),
                (
                    "call_r2",
                    &spawn_wire,
                    r#"{"task":"TASKOTHER work the other branch"}"#,
                ),
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
        "kickoff-refusal one of them will try to peek",
    )
    .expect("status refusal is represented in the turn status");
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "被拒的调用不该把这一轮弄停"
    );

    let root = AgentId::root();
    let peeker = root.child(1);

    // --- 唯一剩下的拒绝：这个 id 不在活树上 ---
    let (absent, is_error) = tool_result(&session, &peeker, "call_a1");
    assert!(is_error, "问一个没 spawn 过的 id 该是 is_error：{absent}");
    assert!(
        absent.contains("root/a9"),
        "拒绝文本该点名是哪个 id：{absent}"
    );
    assert!(
        absent.contains("活 agent 里"),
        "拒绝文本该说清是「不在活树上」而不是方向问题：{absent}"
    );
    // 拒绝文本要给出下一步：现在活着的是哪些。
    assert!(
        absent.contains("root/a2"),
        "拒绝文本该把现在活着的一并给出：{absent}"
    );

    // --- 兄弟：决策 35 之后通得过 ---
    let (sideways, is_error) = tool_result(&session, &peeker, "call_a2");
    assert!(!is_error, "问自己的兄弟现在该成功：{sideways}");
    assert!(
        sideways.contains("root/a2"),
        "兄弟该真的被列出来：{sideways}"
    );
    // 而且它此刻确实活着——否则这条测的是「兄弟恰好不存在」，什么都没证明。
    assert!(
        session.live_agents().contains(&root.child(2)),
        "兄弟该真的活着，否则这条测的是另一件事"
    );

    // --- loop 照常往下走 ---
    let peeker_text: Vec<_> = session
        .messages_of(&peeker)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.to_string()),
            _ => None,
        })
        .collect();
    assert!(
        peeker_text.iter().any(|t| t.contains("carried on anyway")),
        "被拒的 agent 该照常收尾：{peeker_text:#?}"
    );

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
        "父该照常拿到结果：{root_text:#?}"
    );
}
