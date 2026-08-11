//! **117 验收第一条**：029 的并行没有因为「IO 线程 → 并发 future」而退化。
//!
//! # 为什么已有的 `subagent_parallel.rs` 不够
//!
//! 那条断言的是「两个子 agent 的请求在**服务端**的服务区间有交叠」。它盯的是
//! 「两个请求真的同时在飞」，仍然必要（本文件也照抄一份），但它**盯不住 117 换
//! 载体之后新出现的那种退化**：请求是 `provider_call::start` 一起发出去的，底下
//! 各自有一条只读字节的工作线程，所以哪怕泵退化成「一次只推一个 IO future、推到
//! 它跑完为止」，服务端看到的两条服务区间照样重叠——测试全绿，而流式增量实际上
//! 已经变成一条一条串着来了。
//!
//! # 这条盯的是「泵在并发地驱动它们」
//!
//! 判据换成**客户端**这一侧看得见的东西：两个子 agent 的文本增量在事件流里
//! **交替出现**。
//!
//! - 泵并发驱动（`FuturesUnordered` 每次 poll 推一遍所有在飞 future，会合背压让
//!   每个载体最多跑在前面一条）→ 事件流长成 `A B A B A B …`。
//! - 泵一次只推一个（退化）→ 事件流长成 `A A A … B B B …`，**恰好两段**。
//!
//! 所以断言写成「把连续同一个 agent 的增量并成段之后，段数 ≥ 3」：退化的写法在
//! 结构上只能产出 2 段，多一段都产不出来。
//!
//! 脚本让两条流**逐行滴**（`RoutedServer::start_with_line_delay`），两边在同一
//! 段时间里各自有数据可读——不然「谁先被读完」就成了调度运气，断言会时绿时红。
//! 实测形状是 `A B A B A B A B A B A B`（12 条增量 12 段），离阈值 3 很远。

use crate::support;
use std::time::Duration;

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};

use crate::support::routed::{Route, RoutedServer};

/// 每行之间滴这么久。比泵的心跳（20ms）大一档，保证两条流的增量在时间上真的
/// 交错，而不是「一次性到齐、靠 poll 顺序凑出来的交错」。
const DRIP: Duration = Duration::from_millis(30);

fn root_spawns_two() -> Route {
    Route::sse(
        "",
        vec![
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务A：查甲\"}"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务B：查乙\"}"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
            "data: [DONE]",
        ],
    )
}

/// 一个子 agent 的应答：`pieces` 段文本增量（逐行滴由服务器统一负责）。
fn dripping_reply(needle: &'static str, pieces: &[&str]) -> Route {
    let mut lines: Vec<String> = pieces
        .iter()
        .map(|piece| {
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{piece}"}},"finish_reason":null}}]}}"#
            )
        })
        .collect();
    lines.push(
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#
            .to_string(),
    );
    lines.push("data: [DONE]".to_string());
    Route::sse(needle, lines)
}

#[test]
fn two_children_stream_concurrently_instead_of_one_after_another() {
    let dir = support::temp_dir("parallel-futures");
    let server = RoutedServer::start_with_line_delay(
        vec![
            // 父第二跳：子 A 的结论已经进历史，认「甲是」这个 needle。
            Route::sse(
                "甲是",
                vec![
                    r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"都查完了。"},"finish_reason":null}]}"#,
                    r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":80,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":80}}"#,
                    "data: [DONE]",
                ],
            ),
            dripping_reply("任务A", &["甲是", "一", "，", "记", "下", "了"]),
            dripping_reply("任务B", &["乙是", "二", "，", "也", "记", "了"]),
            root_spawns_two(),
        ],
        DRIP,
    );
    let (mut ctx, events) = support::build_ctx_agent_aware(
        server.port,
        &dir,
        ToolTable::builtin().with_spawn(AgentLimits::default()),
    );
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "分头去查甲和乙"));
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // —— 老判据：两个请求真的同时在飞（服务端的服务区间交叠）——————
    assert!(
        server.overlapped("任务A", "任务B"),
        "两个子 agent 的 provider 调用该在时间上重叠：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // —— 新判据：泵在并发地驱动两个 IO future ————————————————
    let child_a = AgentId::new("root/a1");
    let child_b = AgentId::new("root/a2");
    let order: Vec<AgentId> = events
        .borrow()
        .iter()
        .filter(|e| matches!(e.event, RunnerEvent::TextDelta(_)))
        .filter(|e| e.agent == child_a || e.agent == child_b)
        .map(|e| e.agent.clone())
        .collect();
    assert!(
        order.len() >= 8,
        "两个子 agent 一共该吐十来条文本增量，只有 {} 条说明脚本没跑到位：{order:?}",
        order.len()
    );

    let mut runs = 1;
    for pair in order.windows(2) {
        if pair[0] != pair[1] {
            runs += 1;
        }
    }
    assert!(
        runs >= 3,
        "两个子 agent 的增量该在事件流里交替出现（段数 {runs}）。\
         恰好 2 段 = 一个子 agent 的流被整条跑完才轮到另一个 = 泵不再并发驱动 IO future，\
         029 的并行退化成了串行（不报错，只变慢）：{order:?}"
    );
}
