//! 051 验收第一条的后半句 + **决策 35（207）**：`status` 列的是**整棵活树**，
//! 兄弟和祖先都在里面。
//!
//! **这个文件原本断言的是相反的事。** 红线 10 改写之前 `status` 只含调用者的严格
//! 后代，这里断言「祖先不在、兄弟不在」。横读全开之后兄弟看得见兄弟是这一波的
//! 行为核心，于是同一套脚手架现在证明的是更强的一件事。
//!
//! 光「父看得到自己的两个子」证不了这一条——父是 root，它的后代就是整棵树，
//! 放开与否都看不出来。所以调用者取的是**中间那一层**：
//!
//! ```text
//! root
//! ├── root/a1        ← 它调 status
//! │   └── root/a1/a1
//! └── root/a2        ← 兄弟，**此刻正在飞**
//! ```
//!
//! 断言 `root/a1` 看得到全部四个。**兄弟那一条尤其要紧**：这条用例让 root/a2 的
//! 那次 provider 调用慢到 `root/a1` 都发完下一跳了还没回来，并用服务器记的时间窗把
//! 「读树那一刻它确实在飞」断言死——一个还在飞的兄弟也看得见，才说明视野是真的
//! 树而不是「恰好都收敛了的那些」。
//!
//! 同时守住那条**没有**被决策 35 放开的边界：兄弟的 `task` 该出现（那是 activity
//! 那一层的信息），兄弟的**回答正文**仍然不许出现（正文是 collect 的事）。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Session, TurnStatus};
use agent_runtime::run_turn;

use crate::status_indep_support::{
    build_ctx, listed_ids, sse_text, sse_tool_calls, temp_dir, tool_result, wire_tool_name, Route,
    RoutedServer,
};

/// 兄弟那一路的延迟。只要明显长过「root/a1 建孙子 + 读树 + 发下一跳」那一小段
/// 就够，下面的断言比的是服务器记的真实时间窗，不是这个数字本身。
const SIBLING_DELAY: Duration = Duration::from_millis(600);

#[test]
fn a_child_sees_the_whole_tree_including_a_sibling_that_is_still_in_flight() {
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
    )
    .expect("status query should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let root = AgentId::root();
    let left = root.child(1);
    let right = root.child(2);
    let grandchild = left.child(1);

    // 四个 agent 全在树上。
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

    // --- 决策 35：横读全开，整棵树都在 ---
    let (body, is_error) = tool_result(&session, &left, "call_a2");
    assert!(!is_error, "{body}");
    assert_eq!(
        listed_ids(&body),
        vec![
            root.as_str(),
            left.as_str(),
            grandchild.as_str(),
            right.as_str()
        ],
        "root/a1 该看得到整棵活树：祖先 root、自己、孙子、以及兄弟 root/a2：{body}"
    );
    assert!(
        body.lines()
            .any(|l| l.starts_with(&format!("{} ", left.as_str())) && l.ends_with(" (你)")),
        "调用者那一行该标 (你)：{body}"
    );

    // --- 兄弟当时确实在飞 ---
    // root/a1 的第二跳（call_a2 那条路由）发生在 status 结果已经写回之后；它比
    // root/a2 的响应写完还早，说明 status 读树的那一刻 root/a2 正在飞。
    // **这才是这条用例的价值**：一个还在飞的兄弟也看得见，视野就是真的树，
    // 而不是「恰好都收敛了的那些」。
    let sibling = server.call("TASKBRIGHT").expect("兄弟那一路该被调用过");
    let left_hop2 = server.call("call_a2").expect("root/a1 该发过第二跳");
    assert!(
        left_hop2.start < sibling.end,
        "status 读树时兄弟该还在飞：left_hop2.start={:?} sibling.end={:?}",
        left_hop2.start,
        sibling.end,
    );

    // --- 放开的是视野，不是正文 ---
    // 兄弟的 task 该出现（它就是 activity 那一层的信息）……
    assert!(
        body.contains("TASKBRIGHT"),
        "兄弟的任务文本该出现——视野放开了：{body}"
    );
    // ……但兄弟的**回答正文**仍然不许出现。决策 35 §一：`Messages` 在 core 层放行，
    // 工具层不给模型开按槽位读它的入口；正文是 `collect` 的事（ORCHESTRATION §三/五）。
    // 断的是**完整的回答**而不是 `"right branch"`：兄弟的 task 恰好是
    // `"TASKBRIGHT work the right branch"`，拿子串断言会在这里变成一条永远绿的废话
    // ——正是本文件 `listed_ids` 那条注释警告的「假绿灯子串」。
    for answer in ["right branch answer", "grandchild answer", "left branch reported"] {
        assert!(
            !body.contains(answer),
            "任何 agent 的回答正文都不该出现（{answer}）：{body}"
        );
    }

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
