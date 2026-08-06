//! 053 验收 2：`spawn(bg) B`（慢）→ **立刻** `collect(B)` —— collect 槽 `Pending`、
//! 父 `ToolsPending`、泵接着驱动 B、B 落终态后回写、父恢复。
//!
//! 「等价于老的阻塞 spawn，只是显式、择时」这句话在这里被时序钉死：
//!
//! ```text
//! t0  root 第一跳 → spawn(bg) → 当场拿到 agent_id（不等）
//! t0  root 第二跳（collect）发出去时 B **还在跑**        ← 断言一
//! t1  B 答完
//! t2  root 第三跳（带着 collect 结果）才发出去，t2 > t1  ← 断言二
//! ```
//!
//! 中间 root **一次请求都没多发**（总共三跳），所以「父被挡住了」不是靠猜的：
//! 它要是没被挡，t0 到 t1 之间必然会有第三跳。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{run_turn, ToolTable};

use crate::spawn_bg_support::{
    build_ctx, sse_text, sse_tool_call, temp_dir, tool_results, warned_about, wire_tool_name,
    Route, RoutedServer,
};

/// B 慢：足够长，让「root 第三跳在它之后」不是一次毫秒级的巧合。
const CHILD: Duration = Duration::from_millis(400);

#[test]
fn collect_on_a_running_child_parks_the_parent_until_the_child_finishes() {
    let dir = temp_dir("collect-wait");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_collect_b",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("收工：B 说好了"),
        },
        // root 第二跳：**立刻**发 collect —— 这时候 B 才刚起飞。
        Route {
            needle: "call_bg_b",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_collect_b", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "TASKSLOW",
            delay: CHILD,
            status: 200,
            lines: sse_text("ANSWERSLOW 慢子的答案"),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_bg_b",
                &spawn_wire,
                r#"{"task":"TASKSLOW 一件慢活","background":true}"#,
            ),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 开一个慢的后台子，马上就领")
        .expect("collection should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // --- 结果真的回来了，而且是走 collect 那个槽回来的 ---
    let results = tool_results(&session, &AgentId::root());
    assert_eq!(results.len(), 2, "spawn + collect 各一条：{results:#?}");
    assert!(
        results[0].1.contains("root/a1"),
        "第一条是后台 spawn 回的 agent_id：{results:#?}"
    );
    assert_eq!(
        results[1].0, "call_collect_b",
        "第二条该落在 collect 那个 call_id 上"
    );
    assert_eq!(results[1].1, "ANSWERSLOW 慢子的答案");
    assert!(!results[1].2);

    let child = AgentId::new("root/a1");
    assert_eq!(
        session.status_of(&child),
        TurnStatus::Done { truncated: false }
    );

    // --- 时序：collect 发出去时 B 还在跑，父的下一跳在 B 答完之后 ---
    let b = server.call("TASKSLOW").expect("子该被调用");
    let collect_hop = server
        .call("call_bg_b")
        .expect("root 该发出第二跳（collect）");
    let resume_hop = server
        .call("call_collect_b")
        .expect("root 该在拿到结果后恢复");
    assert!(
        collect_hop.start < b.end,
        "collect 该在 B 还在跑的时候就发出去（不然测的是 stash 那条路）：collect={:?} b.end={:?}",
        collect_hop.start,
        b.end,
    );
    assert!(
        resume_hop.start > b.end,
        "父该一直等到 B 落终态才恢复：resume={:?} b.end={:?}",
        resume_hop.start,
        b.end,
    );

    // --- 父在等的那段时间里**一次请求都没多发** ---
    let root_hops = server
        .calls()
        .into_iter()
        .filter(|c| c.needle != "TASKSLOW")
        .count();
    assert_eq!(
        root_hops, 3,
        "root 该正好三跳：kickoff / collect / 收工。多一跳 = 它压根没被挡住"
    );

    // --- 领走了就不是孤儿：轮末没有任何告警 ---
    let events = events.borrow();
    assert!(
        !warned_about(&events, "root/a1"),
        "被领走的子不该在轮末被告警：{events:#?}"
    );
}
