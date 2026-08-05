//! 048 独立测试点：「浏览器连 SSE，模型 spawn 子 agent → 收到 `agent_tree`
//! 帧，其 `nodes` 跟同刻 `GET /sessions/:id/agents` 返回的一致（推和拉两条路
//! 给出同一棵树）」（048 issue 验收原文第一条）。
//!
//! 手法：子 agent 的假上游回复故意延迟一段（`Route::paced`/`.after`），这样
//! SSE 上出现「子已经生出来、还在 `Thinking`」这棵树之后，有一段真实的
//! wall-clock 窗口可以立刻发一次 `GET /sessions/:id/agents`——两条路问的是
//! 同一个瞬间，不是「先后凑巧读到同一个终态」。

mod support;

use std::time::Duration;

use agent_core::{AgentActivity, AgentId, AgentLimits, AgentTree};
use agent_server::{Frame, SessionEvent, ToolTableSpec};

use support::http_client;
use support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

fn text_reply(needle: &'static str, content: &str) -> Route {
    Route::sse(
        needle,
        vec![
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#
            ),
            USAGE_STOP.to_string(),
            "data: [DONE]".to_string(),
        ],
    )
}

/// 跟 [`text_reply`] 一样的形状，只是第一段先等 `delay` 再写——`Route::paced`
/// 是这份支撑代码给「一条路由要故意拖一拖」用的口子（`support::routed` 模块
/// 文档）。
fn delayed_text_reply(needle: &'static str, content: &str, delay: Duration) -> Route {
    let first = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#
    );
    Route::paced(
        needle,
        vec![
            (delay, first.as_str()),
            (Duration::ZERO, USAGE_STOP),
            (Duration::ZERO, "data: [DONE]"),
        ],
    )
}

fn routes() -> Vec<Route> {
    vec![
        // 最具体先判：root 第二跳请求体里带着子的回答文本。
        text_reply("答案X", "最终结论：晴天。"),
        // 子 agent 首跳——故意拖 300ms 再回,给测试留一段窗口在它落终态之前
        // 发一次 GET。
        delayed_text_reply("任务X", "答案X：今天晴天。", Duration::from_millis(300)),
        // 兜底：root 首跳（一次 spawn 调用）。
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务X：查天气\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
                "data: [DONE]",
            ],
        ),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn the_agent_tree_frame_over_sse_matches_a_concurrent_get() {
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
        Some("{\"text\":\"分派任务查天气\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    // 读 SSE 直到看到第一帧「子 agent 已经生出来、还在 Thinking」的
    // `agent_tree`——子的假上游正拖着 300ms 不回，这一刻发 GET 稳稳落在它
    // 落终态之前。
    let mut sse_tree: Option<AgentTree> = None;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let Some(raw) = sse.next_event(Duration::from_secs(5)) else {
            break;
        };
        let frame: Frame =
            serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data));
        if let SessionEvent::AgentTree(tree) = &frame.event
            && tree.nodes.len() == 2
            && tree.nodes[1].activity == AgentActivity::Thinking
        {
            sse_tree = Some(tree.clone());
            break;
        }
    }
    let sse_tree = sse_tree
        .unwrap_or_else(|| panic!("该在子落终态之前看到一帧「root + 子(Thinking)」的 agent_tree"));

    // 同刻（子还在 300ms 的延迟里挂着）发一次 GET——两条路给出的该是完全
    // 相同的一棵树,不是碰巧长得像。
    let get = http_client::request(server.addr, "GET", &format!("/sessions/{id}/agents"), None);
    assert_eq!(get.status, 200, "{}", get.body);
    let get_tree: AgentTree =
        serde_json::from_str(&get.body).unwrap_or_else(|e| panic!("{e}: {}", get.body));

    assert_eq!(
        get_tree, sse_tree,
        "GET /agents 该跟同刻的 SSE agent_tree 帧给出同一棵树"
    );
    assert_eq!(get_tree.nodes[0].id, AgentId::root());
    assert_eq!(get_tree.nodes[1].id, AgentId::root().child(1));
    assert_eq!(get_tree.nodes[1].parent, Some(AgentId::root()));
    assert_eq!(get_tree.nodes[1].task.as_deref(), Some("任务X：查天气"));

    // 收尾：把这一轮排干净，别让子的假上游还挂着连接就结束测试进程。**必须按
    // `frame.agent == root` 过滤**——子 agent 自己落终态（`Thinking` →
    // `Done`）也会广播一条 `Notice::TurnStatusChanged{status: Done}`，标的是
    // 子的 `AgentId`，不是整轮真正收尾的那条（那条标 root）。只看 `status.
    // is_terminal()` 不看 `frame.agent` 会在子先于 root 落终态时提前误判。
    while let Some(raw) = sse.next_event(Duration::from_secs(5)) {
        let frame: Frame =
            serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data));
        let root_terminal = frame.agent == AgentId::root()
            && matches!(
                &frame.event,
                SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal()
            );
        if root_terminal {
            break;
        }
    }

    // 终态之后再问一次 GET：两个都该是 Done——GET 不是只在轮子进行到一半才
    // 好使，它任何时候读到的都是当下的活树。
    let final_get =
        http_client::request(server.addr, "GET", &format!("/sessions/{id}/agents"), None);
    assert_eq!(final_get.status, 200, "{}", final_get.body);
    let final_tree: AgentTree =
        serde_json::from_str(&final_get.body).unwrap_or_else(|e| panic!("{e}: {}", final_get.body));
    assert_eq!(final_tree.nodes.len(), 2);
    assert!(
        matches!(final_tree.nodes[0].activity, AgentActivity::Done { .. }),
        "{final_tree:?}"
    );
    assert!(
        matches!(final_tree.nodes[1].activity, AgentActivity::Done { .. }),
        "{final_tree:?}"
    );
}
