//! 208 验收第 5 条：不暴露任何别的 agent 的东西——正文里不含任何非本 agent
//! 的 id。
//!
//! root 并行 spawn 两个子（root/a1、root/a2），root/a1 一上来就调 `self`。
//! 断言它的正文里不含兄弟 `root/a2` 的 id——**不能**用 `body.contains("root")`
//! 之类的前缀断言（`root/a1` 自己的 id 就以 `root` 打头，那样断言会把自己的
//! id 也当成假阳性），得断整条兄弟 id 的字面串（跟 `status_indep_support` 模块
//! 文档警告的「假绿灯子串」是同一个坑，`root/a2` 不是 `root/a1` 的子串，这条
//! 判据是安全的）。

use std::time::Duration;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::self_indep_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name,
};

#[test]
fn a_childs_self_body_never_names_its_sibling() {
    let dir = temp_dir("self-no-leak");
    let self_wire = wire_tool_name(agent_runtime::SELF_TOOL);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        // root/a1 的第二跳：自读之后收尾。
        Route {
            needle: "call_a1_self",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("a1 branch done"),
        },
        // root 两个 spawn 都拿到结果之后，root 收尾——用两个子各自的回答文本
        // 当 needle，不跟子自己的 call_a1_self 冲突。
        Route {
            needle: "a2 branch done",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("root wrap done"),
        },
        // root/a1 的第一跳：一上来就调 self。
        Route {
            needle: "TASKA1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_a1_self", &self_wire, "{}"),
        },
        // root/a2：什么都不问，直接收尾。
        Route {
            needle: "TASKA2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("a2 branch done"),
        },
        // root 第一跳：并行 spawn 两个子。
        Route {
            needle: "kickoff-no-leak",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_ra1", &spawn_wire, r#"{"task":"TASKA1 一上来就问自己"}"#),
                ("call_ra2", &spawn_wire, r#"{"task":"TASKA2 什么都不问"}"#),
            ]),
        },
    ]);

    let tools = agent_runtime::ToolTable::builtin()
        .with_spawn(agent_core::AgentLimits::default())
        .with_self();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(
        &mut session,
        &mut ctx,
        "kickoff-no-leak 并行两个子，一个自读一个啥都不干",
    )
    .expect("这条流程不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let a1 = root.child(1);
    let a2 = root.child(2);
    assert!(session.live_agents().contains(&a2), "兄弟该真的活着，否则这条测的是另一件事");

    let (body, is_error) = tool_result(&session, &a1, "call_a1_self");
    assert!(!is_error, "纯读不该失败：{body}");
    assert!(
        !body.contains(a2.as_str()),
        "root/a1 的自读正文里不该出现兄弟 {} 的 id：{body}",
        a2.as_str()
    );
}
