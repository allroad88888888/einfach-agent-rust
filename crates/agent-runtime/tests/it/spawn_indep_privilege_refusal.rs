//! 独立测试覆盖点 7：提权拒绝。
//!
//! root 先把 child1 的工具子集**显式收紧**成 `[srv:fs/read, srv:agent/spawn]`
//! （比 root 自己手上的 `[srv:fs/read, srv:fs/list, srv:agent/spawn]` 少一个
//! `srv:fs/list`）。child1 随后尝试 spawn 一个孙子，点名要 `srv:fs/list`——
//! 这个名字在**宿主的完整工具表**里其实存在，但不在 child1 自己被授予的
//! 子集里。断言这是一次**显式拒绝**（`is_error` 文案可辨），不是静默过滤成
//! 一个跟模型以为的不一样的孙子；也不真的建出这个孙子。
//!
//! 只在 root 层面测「父没有的工具」不够：root 自己没有 `ToolsAllowed` 限制，
//! 那样测不出「拿宿主完整表当放行依据」这类 bug——必须测到「授予者自己的
//! 子集」这一层。

mod spawn_indep_support;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use spawn_indep_support::{Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name};

#[test]
fn a_child_cannot_grant_its_own_grandchild_a_tool_it_was_not_itself_granted() {
    let dir = temp_dir("privilege-refusal");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route { needle: "call_child1", delay: Default::default(), status: 200, lines: sse_text("root received child1's report") },
        Route { needle: "call_grandchild_attempt", delay: Default::default(), status: 200, lines: sse_text("child1 done despite the refusal") },
        Route {
            needle: "CHILD1TASK",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_grandchild_attempt",
                &spawn_wire,
                r#"{"task":"GRANDTASK should never run","tools":["srv:fs/list"]}"#,
            ),
        },
        Route {
            needle: "kickoff4",
            delay: Default::default(),
            status: 200,
            lines: sse_tool_call(
                "call_child1",
                &spawn_wire,
                r#"{"task":"CHILD1TASK do restricted work","tools":["srv:fs/read","srv:agent/spawn"]}"#,
            ),
        },
    ]);

    // root 自己手上有 fs/read + fs/list + spawn（没有 shell）。
    let tools = agent_runtime::ToolTable::builtin().with_spawn(AgentLimits::default());
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff4 delegate with a deliberately narrow tool set");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let child1 = root.child(1);

    // 只有 root + child1，没有第三层的孙子。
    let mut live = session.live_agents();
    live.sort();
    let mut expected = vec![root.clone(), child1.clone()];
    expected.sort();
    assert_eq!(live, expected, "被拒绝的孙子不该真的被建出来");

    // child1 自己被授予的子集该恰好是 root 给的那两个，顺序去重后。
    let granted = session.tools_allowed_of(&child1).expect("child1 是被 spawn 出来的，该有 ToolsAllowed");
    let granted_names: Vec<&str> = granted.iter().map(|s| &**s).collect();
    assert!(granted_names.contains(&"srv:fs/read") && granted_names.contains(&"srv:agent/spawn"));
    assert!(!granted_names.contains(&"srv:fs/list"), "child1 不该被授予它自己都没有的 fs/list");

    // child1 尝试提权的那次 spawn 该是显式拒绝：is_error，且不是静默改写
    // 成别的调用——child1 自己的消息历史里只有这一条 tool_result。
    let child1_messages = session.messages_of(&child1);
    let child1_tool_results: Vec<_> = child1_messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, is_error, .. } => Some((content.clone(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(child1_tool_results.len(), 1, "child1 只发起过一次（被拒的）spawn: {child1_messages:#?}");
    assert!(child1_tool_results[0].1, "提权该被显式拒绝，落 is_error: {child1_tool_results:#?}");

    // child1 照常收尾（003 哲学），root 也收到了汇总。
    let root_text: Vec<_> = session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::Text(t) => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert!(root_text.iter().any(|t| t.contains("received child1's report")));
}
