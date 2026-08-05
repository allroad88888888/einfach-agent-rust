//! issue 034 验收：「假上游集成测试：spawn 轮经 HTTP → SSE 帧里两个子 agent
//! 归属交错出现」。
//!
//! 装配手法：`support::routed::RoutedServer`（按请求体路由、一条连接一个
//! 线程，见该模块文档）+ `ToolTableSpec::Full`（034 补的第三档，`srv:agent/
//! spawn` 才连得上 HTTP）。两个子 agent 各自的流式回复用 `Route::paced` 分两段
//! 带节奏地发（A 在 t=0/120ms，B 在 t=60/180ms），wall-clock 上天然交替
//! 抵达——真实的 `AgentServer` 经这条 HTTP 链路把它们各自的 `Frame.agent`
//! （`root/a1`/`root/a2`）原样送到 SSE，交错因此在下行帧序列里也看得见，不是
//! 只有内部事件日志才看得出（`agent-runtime` 的 `subagent_parallel.rs` 证的是
//! 后者，这个文件证的是前者——033 上报的缺口就是「SSE 帧不带归属，看不出两个
//! 子 agent 谁是谁」）。

mod support;

use std::time::Duration;

use agent_core::AgentLimits;
use agent_server::{Frame, SessionEvent, ToolTableSpec};

use support::http_client;
use support::routed::{Route, RoutedServer};

/// root 首跳：一次声明两个 `srv:agent/spawn`。wire 上的函数名是转义过的
/// （`srv:agent/spawn` → `srv_3Aagent_2Fspawn`，`agent-providers/src/wire/
/// names.rs` 的规则），任务文本各自不同——子 agent 的路由靠它区分。
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

/// 一个子 agent 的两段式流式回复：第一段等 `first_delay` 后发（B 额外错开
/// 60ms 好让两边交替），第二段在写完第一段之后再等 120ms。两段拼起来就是
/// `first_chunk`+`second_chunk`——这段文本被塞进 tool_result 送回 root，
/// root 第二跳的路由靠它精确匹配。
fn child_reply(
    needle: &'static str,
    first_chunk: &str,
    second_chunk: &str,
    first_delay: Duration,
) -> Route {
    let first = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{first_chunk}"}},"finish_reason":null}}]}}"#
    );
    let second = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"content":"{second_chunk}"}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}}}"#
    );
    Route::paced(
        needle,
        vec![
            (first_delay, first.as_str()),
            (Duration::from_millis(120), second.as_str()),
            (Duration::ZERO, "data: [DONE]"),
        ],
    )
}

fn routes() -> Vec<Route> {
    vec![
        // 最具体：root 第二跳的请求体里同时带着两个子 agent 的最终结论（已经
        // 变成 tool_result 内容），拿其中一段当 needle 足够唯一。
        Route::sse(
            "甲最终甲结果",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"汇总完毕。"},"finish_reason":"stop"}],"usage":{"prompt_tokens":80,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":80}}"#,
                "data: [DONE]",
            ],
        ),
        // 子 A：t=0 先发一段，120ms 后发第二段（收尾）。
        child_reply("任务A", "甲最终", "甲结果", Duration::ZERO),
        // 子 B：错开 60ms 起步，好让 A/B 的分段在 wall-clock 上交替抵达
        // （期望到达顺序：A1(0) B1(60) A2(120) B2(180)）。
        child_reply("任务B", "乙最终", "乙结果", Duration::from_millis(60)),
        root_spawns_two(),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn spawning_two_children_over_http_interleaves_their_agent_tagged_frames() {
    let upstream = RoutedServer::start(routes());

    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = ToolTableSpec::Full {
        spawn_limits: AgentLimits::default(),
    };
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |c| c,
    )
    .await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201, "{}", create.body);
    let id = support::extract_json_string_field(&create.body, "id");

    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200);

    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some("{\"text\":\"分头去查甲和乙\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    // 收集到终态为止，逐帧解析成 `Frame`（034：agent 归属信封）。
    let mut frames: Vec<Frame> = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while let Some(raw) = sse.next_event(Duration::from_secs(5)) {
        let frame: Frame =
            serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data));
        let terminal = matches!(
            &frame.event,
            SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal()
        );
        frames.push(frame);
        if terminal || std::time::Instant::now() >= deadline {
            break;
        }
    }

    // 两个子 agent 的归属真的到达了 SSE 帧——不是近似（033 的 spawn-activity
    // 近似只能说「疑似有子 agent 在飞」，这里直接读 `frame.agent`）。
    let a1_deltas: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.agent.as_str() == "root/a1" && matches!(f.event, SessionEvent::TextDelta(_))
        })
        .map(|(i, _)| i)
        .collect();
    let a2_deltas: Vec<usize> = frames
        .iter()
        .enumerate()
        .filter(|(_, f)| {
            f.agent.as_str() == "root/a2" && matches!(f.event, SessionEvent::TextDelta(_))
        })
        .map(|(i, _)| i)
        .collect();
    assert!(
        !a1_deltas.is_empty(),
        "该看到 root/a1 的 text_delta 帧：{frames:?}"
    );
    assert!(
        !a2_deltas.is_empty(),
        "该看到 root/a2 的 text_delta 帧：{frames:?}"
    );

    // 交错：至少有一个 root/a2 帧夹在两个 root/a1 帧之间（或反过来）——不只是
    // 「两个都出现过」,而是两段落 wall-clock 上真的交替到达,帧序列本身也交替。
    let interleaved = a1_deltas
        .windows(2)
        .any(|w| a2_deltas.iter().any(|&i| w[0] < i && i < w[1]))
        || a2_deltas
            .windows(2)
            .any(|w| a1_deltas.iter().any(|&i| w[0] < i && i < w[1]));
    assert!(
        interleaved,
        "两个子 agent 的 text_delta 帧该交错出现，实际 a1@{a1_deltas:?} a2@{a2_deltas:?}，全部帧：{frames:?}"
    );

    // 收尾正常：root 第二跳汇总之后落终态。
    let terminal = frames.iter().rev().find_map(|f| match &f.event {
        SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status })
            if status.is_terminal() =>
        {
            Some(status.clone())
        }
        _ => None,
    });
    assert!(
        matches!(terminal, Some(agent_core::TurnStatus::Done { .. })),
        "该以正常结束收尾：{terminal:?}"
    );
}
