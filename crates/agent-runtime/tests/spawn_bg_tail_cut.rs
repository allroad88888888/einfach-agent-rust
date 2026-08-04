//! 052 验收「砍尾有效」：被拆掉的后台子**不会再起下一轮**。
//!
//! 后台子是个**多轮**的（第一跳吐一个 `srv:fs/read` 调用，跑完还要发第二跳）。
//! 两个用例共用同一份脚本，只差 root 第二跳的快慢：
//!
//! | 用例 | root 收尾时机 | 期望 |
//! |---|---|---|
//! | `..._is_cut_short` | 立刻（子第一跳还在飞） | 子被 despawn → 它那条回执撞活性闸 → **工具没执行、第二跳没发出去** |
//! | `..._runs_all_rounds_when_the_parent_is_still_busy` | 1.5s 之后 | 子跑满两轮 —— **证明脚本真的是多轮的**，上面那条不是因为「脚本本来就只有一轮」而绿 |
//!
//! 对照组是这条测试的全部强度所在：没有它，「第二跳没发出去」这句话对一个压根
//! 发不出第二跳的脚本也成立。

mod spawn_bg_support;

use std::time::{Duration, Instant};

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::{RunnerEvent, run_turn};

use spawn_bg_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, wire_tool_name,
};

/// 子第一跳的延迟：留足时间让 root 先收尾。
const CHILD_HOP1: Duration = Duration::from_millis(500);

fn routes(root_hop2: Duration) -> Vec<Route> {
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let read_wire = wire_tool_name("srv:fs/read");
    vec![
        // 子的**第二跳**：请求体里带着它自己那次工具调用的 id。只有子跑满两轮
        // 时这条才会被命中——两个用例的核心断言都是「它有没有被命中」。
        Route { needle: "call_childread", delay: Duration::ZERO, status: 200, lines: sse_text("子的第二轮答完了") },
        // root 的第二跳：带着 spawn 的 call_id。
        Route { needle: "call_bg", delay: root_hop2, status: 200, lines: sse_text("root 收尾") },
        // 子的第一跳：吐一个本地工具调用（跑完它就会发第二跳）。
        Route {
            needle: "TAILTASK",
            delay: CHILD_HOP1,
            status: 200,
            lines: sse_tool_call("call_childread", &read_wire, r#"{"path":"note.txt"}"#),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_bg", &spawn_wire, r#"{"task":"TAILTASK 多轮后台活","background":true}"#),
        },
    ]
}

fn run(dir_tag: &str, root_hop2: Duration) -> (RoutedServer, Session, Vec<agent_runtime::AgentEvent>, TurnStatus, Duration) {
    let dir = temp_dir(dir_tag);
    std::fs::write(dir.join("note.txt"), "一行料\n").unwrap();
    let server = RoutedServer::start(routes(root_hop2));
    let tools = agent_runtime::ToolTable::builtin().with_spawn(agent_core::AgentLimits::default());
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let start = Instant::now();
    let status = run_turn(&mut session, &mut ctx, "kickoff 一个多轮后台子");
    let elapsed = start.elapsed();
    let events = events.borrow().clone();
    (server, session, events, status, elapsed)
}

/// 砍尾：root 立刻收尾 → 孤儿被拆 → 它第一跳的回执被活性闸丢掉 → **工具没跑、
/// 第二跳没发**。
#[test]
fn a_reaped_background_child_is_cut_short() {
    let (server, session, events, status, elapsed) = run("bg-tail-cut", Duration::ZERO);

    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(elapsed < Duration::from_secs(8), "该在有界时间内收尾：实际 {elapsed:?}");

    assert!(
        server.call("call_childread").is_none(),
        "被拆掉的子不该再起第二跳：{:#?}",
        server.bodies()
    );
    assert!(
        !events.iter().any(|e| matches!(
            &e.event,
            RunnerEvent::ToolExecuting { request, .. } if &*request.tool == "srv:fs/read"
        )),
        "死 agent 的工具调用不该被执行：{events:#?}"
    );
    assert!(!session.is_live(&AgentId::new("root/a1")), "孤儿该已经非活");
}

/// 对照组：root 还在忙的时候，同一个子跑满两轮 —— 上面那条断言不是空的。
#[test]
fn the_same_child_runs_all_rounds_when_the_parent_is_still_busy() {
    let (server, session, _events, status, _elapsed) =
        run("bg-tail-control", Duration::from_millis(1500));

    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(
        server.call("call_childread").is_some(),
        "没被拆的子该跑满两轮（否则砍尾那条测试是空跑的）：{:#?}",
        server.bodies()
    );
    assert_eq!(
        session.status_of(&AgentId::new("root/a1")),
        TurnStatus::Done { truncated: false },
        "子该自己跑到终态"
    );
}
