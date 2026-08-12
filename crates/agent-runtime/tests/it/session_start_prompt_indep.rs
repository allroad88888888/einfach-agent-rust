//! 独立测试：issue 135（docs/issues/135-session-start-driver.md）验收里
//! 「进 prompt」那一条——`run_session_start` 产出的前缀块，真的经 `run_turn`
//! 到达发给 provider 的 wire 请求体的 system 段，且逐轮字节稳定（红线 11）。
//! 只依据「验收」/「注意」两节 + `docs/INVARIANTS.md` 红线 11 写成，**不看**
//! `crates/agent-runtime/src/session_start.rs` / `subagent.rs` / `runner.rs`
//! 里的实现体，也不看 `provider_call.rs` 怎么组 `Ingredients`。
//!
//! `run_session_start` 自己的契约（顺序/空文本/失败/恢复不重跑）在
//! `session_start_indep.rs` 里，那四条都不碰 provider/wire，本文件只管这一条。
//!
//! **选路**：走既有 fake-provider 设施（`support::spawn_recording_server` +
//! `support::build_ctx_with` + `agent_runtime::run_turn`）拿真实 wire 请求体，
//! 不退到 Ingredients 层——`spawn_recording_server` 是这个 crate 已有的、专门给
//! 独立测试捕获发出去的原始请求体字节用的设施（`transient_source_chain.rs` /
//! `send_plan_wiring_undo_restores_bytes.rs` 等已经在用），它跑得通、离
//! 「真正发给 provider 的字节」最近，没有理由绕道自己拼 `Ingredients`。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use agent_core::{AgentId, AgentLimits, Session, ToolSpec, TurnStatus};
use agent_runtime::{run_session_start, run_turn, CallTiming, TimedRun, ToolTable};
use serde_json::json;

use crate::support;
use crate::support::routed::{Route, RoutedServer};

fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// 总是成功、回一段固定文本的执行体。
fn ok_text(text: &'static str) -> TimedRun {
    Box::new(
        move |_table: &ToolTable, _input: &serde_json::Value| -> Result<Arc<str>, Arc<str>> {
            Ok(Arc::from(text))
        },
    )
}

/// 断言：`run_session_start` 产出的 init 块文本，出现在两轮请求体的 system 段里，
/// 且**恰好一次**——不是零次（没进 prompt）也不是两次以上（前缀不稳定，
/// 红线 11 的经典症状：功能一切正常，只是每一轮都全价）。
#[test]
fn init_chunk_text_reaches_the_wire_system_segment_exactly_once_per_round() {
    const MARKER: &str = "SESSION-START-MARKER-af31c9";

    let dir = support::temp_dir("session-start-indep-into-prompt");
    let tools = ToolTable::builtin().with_timed(
        spec("alpha", "把标记文本塞进开局前缀"),
        CallTiming::SessionStart,
        ok_text(MARKER),
    );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("唯一的工具该成功");

    let (port, bodies) = support::spawn_recording_server(vec![
        support::sse_text("第一轮回复"),
        support::sse_text("第二轮回复"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(port, &dir, tools);

    run_turn(&mut session, &mut ctx, "第一句话").expect("第一轮不该是 source failure");
    session.begin_turn();
    run_turn(&mut session, &mut ctx, "第二句话").expect("第二轮不该是 source failure");

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "两轮各录到一条请求体");

    for (round, body) in bodies.iter().enumerate() {
        let system_text = wire_system_text(body);
        let occurrences = system_text.matches(MARKER).count();
        assert_eq!(
            occurrences, 1,
            "第 {round} 轮：init 块的文本该恰好出现一次在 system 段，实际 {occurrences} 次。\
             system 段：{system_text}"
        );
    }
}

/// 请求体里那条 `role: "system"` 消息的正文——模型真正看到的那串字符。
/// 照 `host_skills_index_is_byte_deterministic.rs::wire_system_text` 的同款手法，
/// 只是这里从 `spawn_recording_server` 录到的原始字符串取，不是从
/// `agent_providers::Encoded::body` 取。
fn wire_system_text(body: &str) -> String {
    let value: serde_json::Value = serde_json::from_str(body).expect("请求体该是合法 JSON");
    let messages = value["messages"].as_array().expect("请求体里该有 messages");
    messages
        .iter()
        .find(|m| m["role"] == "system")
        .expect("该有一条 system 消息")["content"]
        .as_str()
        .expect("system 消息该有文本正文")
        .to_string()
}

/// 145 §做什么 第 5 条的看门狗：spawn 两个子 agent、开局工具的执行计数仍
/// 是 1。跟本文件上一条测试同一个假设（`run_session_start` 只在新建会话时
/// 跑一次），这条把断言从「只 spawn 一个 agent」扩到「spawn 两个」——`system_for`
/// 每一跳都要重新组一遍 system，如果谁把 145 的前缀过滤实现成了「按名单重新
/// 跑一遍 timed 工具」而不是「过滤缓存值」，这里就会红（`subagent_tests.rs`
/// 已经用纯单元测试盯过 `filter_prefix_chunks` 本身不重跑；这条从 wire 层面
/// 再钉一次，覆盖真实 `run_turn` 循环）。
///
/// 计数器真正被算一次的时刻是 `run_session_start`（调用发生在 `run_turn` 之
/// 前），之后两个子 agent 各跑一整轮、根 agent 收完两份结果再收尾——全程不该
/// 再碰这个执行体。两个子按**顺序**（不是并行）spawn 出来就够用：这条盯的是
/// 「组 system/发请求这条路会不会偷跑 timed 执行体」，跟两个子是不是同时在飞
/// 无关（那是 `spawn_indep_sibling_prefix.rs`/`subagent_parallel.rs` 盯的另一
/// 件事）。
#[test]
fn spawning_two_children_across_a_round_runs_session_start_exactly_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);

    let dir = support::temp_dir("session-start-watchdog-two-children");
    let tools = ToolTable::builtin()
        .with_spawn(AgentLimits::default())
        .with_timed(
            spec("alpha", "开局工具，执行会被计数"),
            CallTiming::SessionStart,
            Box::new(move |_table: &ToolTable, _input: &serde_json::Value| {
                counted.fetch_add(1, Ordering::SeqCst);
                Ok::<Arc<str>, Arc<str>>(Arc::from("INDEX-TEXT"))
            }),
        );

    let mut session = Session::new(AgentId::root());
    run_session_start(&mut session, &tools).expect("唯一的工具该成功");
    assert_eq!(calls.load(Ordering::SeqCst), 1, "新建会话该执行一次");

    // 路由按「先查最晚才会出现的 needle」排序（`RoutedServer` 首次匹配即用，
    // 越具体越要排在前面）：根的每一跳请求体都累积着此前全部历史，越新的
    // call_id 是区分「这是第几跳」唯一可靠的东西——同 `spawn_indep_depth_chain.
    // rs` 的手法。
    let server = RoutedServer::start(vec![
        text_call_route("call_b", "both children reported, chain complete"),
        text_call_route("CHILDB-MARK", "child B finished successfully"),
        spawn_call_route("call_a", "call_b", "CHILDB-MARK second child task"),
        text_call_route("CHILDA-MARK", "child A finished successfully"),
        spawn_call_route("startchain", "call_a", "CHILDA-MARK first child task"),
    ]);
    let (mut ctx, _events) = support::build_ctx_with(server.port, &dir, tools);

    let status = run_turn(
        &mut session,
        &mut ctx,
        "startchain please spawn two children, one after another",
    )
    .expect("spawning two children in sequence should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let mut live = session.live_agents();
    live.sort();
    let root = AgentId::root();
    let mut expected = vec![root.clone(), root.child(1), root.child(2)];
    expected.sort();
    assert_eq!(live, expected, "该恰好三个 agent：root + 两个子");

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "spawn 两个子 agent 并跑完一轮之后，开局工具执行计数该仍是 1"
    );
}

/// DeepSeek wire：一条 `srv:agent/spawn` 工具调用响应（hop1）。手法照抄本文件
/// 顶部 `ok_text`/`spec` 那一层的风格，只是这里要吐一个真的 tool_calls 帧，
/// `support::sse_tool_call` 返回的是 `ScriptedResponse`（给 `spawn_scripted_
/// server` 用，逐次连接对应逐条脚本），这里要的是 `Route::sse` 吃的
/// `Vec<String>`，所以不复用它，直接拼——跟 `spawn_indep_support::sse_tool_
/// call` 同一个形状。
fn spawn_call_route(needle: &'static str, call_id: &str, task: &str) -> Route {
    let arguments = json!({ "task": task }).to_string();
    let chunk1 = json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": { "name": "srv_3Aagent_2Fspawn", "arguments": arguments }
                }]
            },
            "finish_reason": null
        }]
    });
    let chunk2 = json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    Route::sse(
        needle,
        vec![
            format!("data: {chunk1}"),
            format!("data: {chunk2}"),
            "data: [DONE]".to_string(),
        ],
    )
}

/// DeepSeek wire：一条 `EndTurn` 纯文本应答，`Route::sse` 版本（同上，`support::
/// sse_text` 吐的是 `ScriptedResponse`，这里要 `Vec<String>`）。
fn text_call_route(needle: &'static str, text: &str) -> Route {
    let chunk1 = json!({
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let chunk2 = json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    Route::sse(
        needle,
        vec![
            format!("data: {chunk1}"),
            format!("data: {chunk2}"),
            "data: [DONE]".to_string(),
        ],
    )
}
