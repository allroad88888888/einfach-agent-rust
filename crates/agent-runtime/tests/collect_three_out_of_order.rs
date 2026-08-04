//! 053 验收 3：`spawn(bg) A,B,C` → **按谁先完先领**（顺序跟 spawn 顺序不同）→
//! 三份结果都拿到；全部领完之后 detached 名单和 stash 都空 → **轮末孤儿收尾不
//! 触发、一句告警都没有**。
//!
//! 最后那半条是 052 留给本 issue 的接力点的验收：`Subtree::take_orphans` 的第三条
//! 判据（「没有 collect 绑定」）在 052 里恒真，053 接上之后它真的会挡人。这里用
//! 「一条 `RunnerEvent::OrphanedChild` 都没有」来钉（054 之前借的是
//! `TransportTrouble`）——052 的三个用例正好相反，那边每一个没被领的后台子都
//! 留下一句告警。
//!
//! 领取顺序是 **B（最快）→ C（中）→ A（最慢）**，跟 spawn 顺序 A,B,C 反着来。
//! 这正是本里程碑要买的那件事：先收先完成的，别按发出去的顺序死等第一个。

mod spawn_bg_support;

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use spawn_bg_support::{
    Route, RoutedServer, build_ctx, orphan_warnings, sse_text, sse_tool_call, sse_tool_calls,
    temp_dir, tool_results, wire_tool_name,
};

const SLOW: Duration = Duration::from_millis(400);
const MEDIUM: Duration = Duration::from_millis(150);

#[test]
fn three_background_children_are_collected_fastest_first_and_nothing_is_left_to_reap() {
    let dir = temp_dir("collect-three");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        // 越靠后发生的 call_id 越靠前判（root 每一跳都带着此前全部 call_id）。
        Route { needle: "call_c1", delay: Duration::ZERO, status: 200, lines: sse_text("三个都拿到了") },
        Route {
            needle: "call_c3",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c1", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "call_c2",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c3", &collect_wire, r#"{"id":"root/a3"}"#),
        },
        Route {
            needle: "call_bg_1",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_c2", &collect_wire, r#"{"id":"root/a2"}"#),
        },
        Route { needle: "TASKONE", delay: SLOW, status: 200, lines: sse_text("ANSWERONE 甲") },
        Route { needle: "TASKTWO", delay: Duration::ZERO, status: 200, lines: sse_text("ANSWERTWO 乙") },
        Route { needle: "TASKTHREE", delay: MEDIUM, status: 200, lines: sse_text("ANSWERTHREE 丙") },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_calls(&[
                ("call_bg_1", &spawn_wire, r#"{"task":"TASKONE 最慢的一件","background":true}"#),
                ("call_bg_2", &spawn_wire, r#"{"task":"TASKTWO 最快的一件","background":true}"#),
                ("call_bg_3", &spawn_wire, r#"{"task":"TASKTHREE 中间那件","background":true}"#),
            ]),
        },
    ]);

    let tools = ToolTable::builtin().with_spawn(AgentLimits::default()).with_status().with_collect();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 三件事一起拆出去");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // --- 三个 agent_id + 三份答案，各就各位 ---
    let results = tool_results(&session, &AgentId::root());
    assert_eq!(results.len(), 6, "三次 spawn + 三次 collect：{results:#?}");
    for (call_id, body, is_error) in results.iter().take(3) {
        assert!(!is_error, "后台 spawn 该当场成功：{call_id} {body}");
        assert!(body.contains("agent_id"), "前三条该是 agent_id：{body}");
    }
    let collected: Vec<(String, String)> =
        results[3..].iter().map(|(id, body, _)| (id.clone(), body.clone())).collect();
    assert_eq!(
        collected,
        vec![
            ("call_c2".to_string(), "ANSWERTWO 乙".to_string()),
            ("call_c3".to_string(), "ANSWERTHREE 丙".to_string()),
            ("call_c1".to_string(), "ANSWERONE 甲".to_string()),
        ],
        "该按「谁先完先领」的顺序各自落在自己的 call_id 上，三份都不缺",
    );
    assert!(results[3..].iter().all(|(_, _, is_error)| !is_error));

    // 三个子都真的跑完了（上面那三条不是靠别的路混过去的）。
    for i in 1..=3 {
        let child = AgentId::new(format!("root/a{i}"));
        assert_eq!(session.status_of(&child), TurnStatus::Done { truncated: false }, "{child:?}");
        assert!(session.is_live(&child), "领完的子还活着——collect 不拆人");
    }

    // --- 全领完 → 两张表都空 → 轮末清算什么都没做，一句告警都没有 ---
    let events = events.borrow();
    let warnings = orphan_warnings(&events);
    assert!(
        warnings.is_empty(),
        "全部领完就不该有孤儿、也不该有「跑完没人领」——`take_orphans` 的第三条判据\
         这一下才真的在挡人：{warnings:#?}"
    );
}
