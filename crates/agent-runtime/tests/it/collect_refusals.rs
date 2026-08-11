//! 053 验收：三种领不动的情形各回一条 `is_error` 的 tool_result，**loop 照常
//! 往下跑**（决策 20 的兜底哲学：把话说清楚，让模型自己收敛），一次 panic、一次
//! 卡住都没有。
//!
//! 一条脚本走完三种：
//!
//! 1. **双重 collect**：同一个 id 领第二次 —— 一份结果只能领一次（领取即消费）。
//! 2. **不存在的 id**：形状对（是自己的后代）但从来没 spawn 出来过。
//! 3. **不是后代**（这里用调用者自己）—— 红线 10，拒绝文本还要把「你能领的是
//!    哪些」一并给出。
//!
//! 最后 root 照样答完这一轮：四条 tool_result 里三条 `is_error`，`run_turn` 返回
//! `Done`。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use crate::spawn_bg_support::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, temp_dir, tool_results, wire_tool_name,
};

/// 让 A 有时间答完：第一次 collect 因此走 stash 那条路，第二次才是货真价实的
/// 「领过了」而不是「还没跑完」。
const ROOT_HOP2: Duration = Duration::from_millis(250);

#[test]
fn collecting_twice_an_unknown_id_or_a_non_descendant_all_come_back_as_errors() {
    let dir = temp_dir("collect-refusals");
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_self",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("好吧，剩下的我自己答"),
        },
        Route {
            needle: "call_ghost",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_self", &collect_wire, r#"{"id":"root"}"#),
        },
        Route {
            needle: "call_dup",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_ghost", &collect_wire, r#"{"id":"root/zz"}"#),
        },
        Route {
            needle: "call_ok",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_dup", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "call_bg_r",
            delay: ROOT_HOP2,
            status: 200,
            lines: sse_tool_call("call_ok", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "TASKR",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("ANSWERR 领一次就好"),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call(
                "call_bg_r",
                &spawn_wire,
                r#"{"task":"TASKR 一件小事","background":true}"#,
            ),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(
        &mut session,
        &mut ctx,
        "kickoff 开一个，然后乱领一通",
    ));

    // 三次拒绝一次都没有打断这一轮。
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "拒绝是给模型看的文本，不是这一轮的结局"
    );

    let results = tool_results(&session, &AgentId::root());
    assert_eq!(
        results.len(),
        5,
        "spawn + 一次成功的 collect + 三次拒绝：{results:#?}"
    );

    // 第一次领：成功。
    assert_eq!(results[1].0, "call_ok");
    assert!(!results[1].2, "第一次该领得到：{results:#?}");
    assert_eq!(results[1].1, "ANSWERR 领一次就好");

    // 第二次领同一个：领过了。
    assert_eq!(results[2].0, "call_dup");
    assert!(results[2].2, "同一份结果不该被领第二次：{results:#?}");
    assert!(
        results[2].1.contains("root/a1"),
        "拒绝文本该点名是哪个：{}",
        results[2].1
    );
    assert!(
        results[2].1.contains("只能领一次"),
        "该说清为什么：{}",
        results[2].1
    );

    // 根本不存在的 id。
    assert_eq!(results[3].0, "call_ghost");
    assert!(results[3].2, "不存在的 id 该是 is_error：{results:#?}");
    assert!(results[3].1.contains("root/zz"), "{}", results[3].1);

    // 不是后代（这里是它自己）——红线 10。
    assert_eq!(results[4].0, "call_self");
    assert!(
        results[4].2,
        "领自己该被拒（红线 10 一条例外都没有）：{results:#?}"
    );
    assert!(
        results[4].1.contains("后代"),
        "拒绝文本该说清规则：{}",
        results[4].1
    );

    // 子还好好地活着、也确实干完了：上面那些拒绝不是因为世界塌了。
    let child = AgentId::new("root/a1");
    assert!(session.is_live(&child));
    assert_eq!(
        session.status_of(&child),
        TurnStatus::Done { truncated: false }
    );
}
