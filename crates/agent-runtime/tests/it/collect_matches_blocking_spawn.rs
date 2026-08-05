//! 053 验收 1：`spawn(bg) A` → `status` 看到 A Done → `collect(A)` **立刻**拿到
//! A 的最终结果；而且那份结果**跟同一个任务用阻塞 spawn 拿到的逐字节相同**。
//!
//! 第二条是本 issue 语义上的钉子：ORCHESTRATION §三 那句
//! 「前台 spawn ≡ spawn(bg) 紧跟 collect」不是修辞。两条路只差「模型什么时候把
//! 等待这一笔记上」，回到父历史里的字节必须一模一样——不一样就说明后台这条路上
//! 有人多写/少写了什么。
//!
//! 夹具复用 052 的 `spawn_bg_support`（它自己又复用 029 的并发假服务器）：collect
//! 是后台那半边的另一头，同一种服务器形状，没有一处需要为它另起一份。

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{run_turn, AgentEvent, ToolTable};

use crate::spawn_bg_support::{
    build_ctx, sse_text, sse_tool_call, temp_dir, tool_results, warned_about, wire_tool_name,
    Route, RoutedServer,
};

/// 两条路跑的是**同一个任务**、拿的是**同一份回答**——只有这样，最后那句
/// 「内容相同」才是在比较两条路径，而不是在比较两段互不相干的文本。
const TASK: &str = "TASKCOLLECT 数一数 a.txt 有几行";
const ANSWER: &str = "ANSWERCOLLECT 三行，最后一行没有换行符";

/// 两条路的 spawn 入参**只差一个 `background`**，任务文本共用同一个常量：
/// 「两条路跑的是同一件事」因此是结构事实，不靠两处字符串手抄对齐。
fn background_input() -> String {
    format!(r#"{{"task":"{TASK}","background":true}}"#)
}

fn blocking_input() -> String {
    format!(r#"{{"task":"{TASK}"}}"#)
}

/// 让 A 有时间答完再让 root 醒来：A 的延迟是 0，root 第二跳 300ms。于是 root 发
/// `status` 的时候 A 早就落终态进 stash 了——`collect` 那一步是从 stash 里端走，
/// 不是等出来的。
const ROOT_HOP2: Duration = Duration::from_millis(300);

/// 后台那条路：spawn(bg) → status → collect。返回 collect 拿到的那段正文 +
/// 这一轮发出去的全部事件（轮末有没有告警要看它）+ 服务器（时序要看它）。
fn background_run(tag: &str) -> (String, Vec<AgentEvent>, RoutedServer) {
    let dir = temp_dir(tag);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);
    let status_wire = wire_tool_name(agent_runtime::STATUS_TOOL);
    let collect_wire = wire_tool_name(agent_runtime::COLLECT_TOOL);

    // **越具体的 needle 越靠前**：root 每一跳的请求体都含着此前全部 call_id
    // （tool_call_id 回填），只有按「最新那个」先判才认得出是第几跳。
    let server = RoutedServer::start(vec![
        Route {
            needle: "call_collect_a",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("收工：A 说三行"),
        },
        Route {
            needle: "call_status",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_collect_a", &collect_wire, r#"{"id":"root/a1"}"#),
        },
        Route {
            needle: "call_bg_a",
            delay: ROOT_HOP2,
            status: 200,
            lines: sse_tool_call("call_status", &status_wire, "{}"),
        },
        Route {
            needle: "TASKCOLLECT",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text(ANSWER),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_bg_a", &spawn_wire, &background_input()),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (mut ctx, events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 开个后台的");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let results = tool_results(&session, &AgentId::root());
    assert_eq!(
        results.len(),
        3,
        "spawn + status + collect 各一条：{results:#?}"
    );
    assert!(
        results[0].1.contains("root/a1"),
        "第一条该是后台 spawn 回的 agent_id：{results:#?}"
    );
    assert!(
        results[1].1.contains("root/a1") && results[1].1.contains("Done"),
        "collect 之前 status 该已经看到 A 干完了（这条测的是「立刻」的前提）：{results:#?}"
    );

    let (call_id, body, is_error) = results[2].clone();
    assert_eq!(call_id, "call_collect_a");
    assert!(!is_error, "子干成了，collect 不该是 is_error：{body}");
    (body, events.borrow().clone(), server)
}

/// 前台那条路：一次普通的阻塞 spawn，同一个任务、同一份回答。
fn blocking_run(tag: &str) -> String {
    let dir = temp_dir(tag);
    let spawn_wire = wire_tool_name(agent_runtime::SPAWN_TOOL);

    let server = RoutedServer::start(vec![
        Route {
            needle: "call_fg_a",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text("收工：A 说三行"),
        },
        Route {
            needle: "TASKCOLLECT",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_text(ANSWER),
        },
        Route {
            needle: "kickoff",
            delay: Duration::ZERO,
            status: 200,
            lines: sse_tool_call("call_fg_a", &spawn_wire, &blocking_input()),
        },
    ]);

    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_status()
        .with_collect();
    let (mut ctx, _events) = build_ctx(server.port, &dir, tools);
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "kickoff 开个前台的");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let results = tool_results(&session, &AgentId::root());
    assert_eq!(
        results.len(),
        1,
        "阻塞 spawn 只有一条 tool_result：{results:#?}"
    );
    assert!(!results[0].2);
    results[0].1.clone()
}

/// 已经跑完的后台子 → collect **当场**拿到它的回答，不多跑一次 provider 调用，
/// 轮末也没有「没人领」的告警（领了）。
#[test]
fn collect_hands_over_a_finished_background_childs_answer_at_once() {
    let (body, events, server) = background_run("collect-stash-hit");

    assert_eq!(body, ANSWER, "collect 该原样交出子的最后一段文本");

    // 「立刻」的操作证据：子只被调用过一次（collect 没有把它再驱动一轮），而且
    // 它答完的时刻**早于** root 发出 status 那一跳——collect 无从等起。
    let child_calls = server
        .calls()
        .into_iter()
        .filter(|c| c.needle == "TASKCOLLECT")
        .count();
    assert_eq!(
        child_calls, 1,
        "子该只跑一次；collect 是领结果，不是再跑一遍"
    );
    let child = server.call("TASKCOLLECT").expect("子该被调用");
    let status_hop = server.call("call_status").expect("status 那一跳该发出去");
    assert!(
        child.end < status_hop.start,
        "子该在 root 发 status 之前就答完了：child.end={:?} status_hop.start={:?}",
        child.end,
        status_hop.start,
    );

    assert!(
        !warned_about(&events, "root/a1"),
        "领走了的后台子不该在轮末被告警（stash 该已经空了）：{events:#?}"
    );
}

/// **后台 = 前台拆开**：同一个任务、同一份回答，collect 交给父的字节跟阻塞 spawn
/// 交给父的字节完全相同。两条路要是哪天分了叉（多包一层、少一行说明、加个前缀），
/// 这一条立刻红。
#[test]
fn and_it_is_byte_for_byte_what_blocking_spawn_would_have_returned() {
    let (background, _events, _server) = background_run("collect-vs-blocking-bg");
    let blocking = blocking_run("collect-vs-blocking-fg");

    assert_eq!(
        background, blocking,
        "spawn(bg)+collect 和阻塞 spawn 该回同一份正文"
    );
}
