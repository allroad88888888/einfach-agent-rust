//! 051 验收第一条的后半句 + **红线 10**：`status` 只含调用者的后代，兄弟和祖先
//! 的分支一个都不出现。
//!
//! 光「父看得到自己的两个子」证不了这一条——父是 root，它的后代就是整棵树，
//! 收窄错了也看不出来。所以调用者取的是**中间那一层**：
//!
//! ```text
//! root
//! ├── root/a1        ← 它调 status
//! │   └── root/a1/a1
//! └── root/a2        ← 兄弟，**此刻正在飞**
//! ```
//!
//! 断言 `root/a1` 只看得到 `root/a1/a1`：祖先（root）不在、兄弟（root/a2）不在。
//! 兄弟不在**不是因为它不存在**——这条用例让 root/a2 的那次 provider 调用慢到
//! `root/a1` 都发完下一跳了还没回来，并用服务器记的时间窗把这件事断言死。否则
//! 「兄弟没出现」和「兄弟还没被建出来」在结果上长得一模一样，红线 10 破了也是绿的。

mod status_indep_support;

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use status_indep_support::{
    Route, RoutedServer, build_ctx, listed_ids, sse_text, sse_tool_calls, temp_dir, tool_result,
    wire_tool_name,
};

/// 兄弟那一路的延迟。只要明显长过「root/a1 建孙子 + 读树 + 发下一跳」那一小段
/// 就够，下面的断言比的是服务器记的真实时间窗，不是这个数字本身。
const SIBLING_DELAY: Duration = Duration::from_millis(600);

#[test]
fn a_child_sees_only_its_own_descendants_never_its_running_sibling_or_its_ancestor() {
    let dir = temp_dir("status-scope");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);

    let server = RoutedServer::start(vec![
        // 按「越具体越靠前」排：root/a1 的第二跳请求体里同时有 call_a2、TASKA1、
        // TASKALEFT 三个 needle，靠这条排在最前面认领。
        Route {
            needle: "call_a2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("left branch reported"),
        },
        Route {
            needle: "call_r1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("all done"),
        },
        Route {
            needle: "TASKA1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("grandchild answer"),
        },
        Route {
            needle: "TASKALEFT",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_a1",
                    &spawn_wire,
                    r#"{"task":"TASKA1 dig one level deeper"}"#,
                ),
                ("call_a2", &status_wire, "{}"),
            ]),
        },
        Route {
            needle: "TASKBRIGHT",
            delay: SIBLING_DELAY,
            status: 200,
            lines: sse_text("right branch answer"),
        },
        Route {
            needle: "kickoff-scope",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                (
                    "call_r1",
                    &spawn_wire,
                    r#"{"task":"TASKALEFT work the left branch"}"#,
                ),
                (
                    "call_r2",
                    &spawn_wire,
                    r#"{"task":"TASKBRIGHT work the right branch"}"#,
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
        "kickoff-scope two branches, the left one goes deeper",
    );
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let left = root.child(1);
    let right = root.child(2);
    let grandchild = left.child(1);

    // 四个 agent 全在树上——「兄弟没出现」不能是因为它压根不存在。
    let mut live = session.live_agents();
    live.sort();
    assert_eq!(
        live,
        vec![
            root.clone(),
            left.clone(),
            grandchild.clone(),
            right.clone()
        ]
    );

    // --- 红线 10：只下读 ---
    let (body, is_error) = tool_result(&session, &left, "call_a2");
    assert!(!is_error, "{body}");
    assert_eq!(
        listed_ids(&body),
        vec![grandchild.as_str()],
        "root/a1 只该看得到自己那一支：祖先 root 和兄弟 root/a2 一个都不许在里面：{body}"
    );

    // --- 兄弟当时确实在飞 ---
    // root/a1 的第二跳（call_a2 那条路由）发生在 status 结果已经写回之后；它比
    // root/a2 的响应写完还早，说明 status 读树的那一刻 root/a2 正在飞。
    let sibling = server.call("TASKBRIGHT").expect("兄弟那一路该被调用过");
    let left_hop2 = server.call("call_a2").expect("root/a1 该发过第二跳");
    assert!(
        left_hop2.start < sibling.end,
        "status 读树时兄弟该还在飞：left_hop2.start={:?} sibling.end={:?}",
        left_hop2.start,
        sibling.end,
    );

    // --- 兄弟的任何痕迹都不该漏进来 ---
    assert!(
        !body.contains("TASKBRIGHT"),
        "兄弟的任务文本不该出现：{body}"
    );
    assert!(
        !body.contains("right branch"),
        "兄弟的正文更不该出现：{body}"
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
        "整棵树该正常收工：{root_text:#?}"
    );
}
