//! 060 验收第二条的**生产形态**：Web 宿主拿到 `ToolExecuting` 之后**永不回传**，
//! 会话不该永久停在 `ToolsPending`。
//!
//! 这是 060 现象二真正咬人的地方。`web_tool_result_resumes_turn.rs` 覆盖的是闭环
//! （宿主回传 → 同一轮接着跑）；这里把回传整条掐掉——前端崩了 / 网关挂了 / 客户端
//! 压根没实现这个工具。actor 派出远端调用后 `run_turn` 就返回了，控制权回到命令
//! 队列，060 之前那句 `rx.recv()` 是**无限期**阻塞：没有回传、没有取消，这个
//! session 就再也不会动一下，且全程不报错。
//!
//! 现在 actor 空闲等命令时至多等到最早的那条远端截止线
//! （`actor::body::next_command`），到点让 runtime 把槽判失败。模型收到 `is_error`
//! 自己收敛，本轮正常落终态。
//!
//! 截止线压到 300ms（`SessionTemplate::remote_tool_timeout`）——真实默认是 10 分钟。

use crate::support;
use std::time::Duration;

use agent_server::{Frame, SessionEvent, ToolTableSpec};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

fn browser_action_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_browser_1","type":"function","function":{"name":"browser_action","arguments":"{\"action\":\"render_card\",\"payload\":{\"title\":\"Hello\"}}"}}]},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader) -> Frame {
    let raw = sse.next_event(Duration::from_secs(5)).expect("该收到一帧");
    serde_json::from_str(&raw.data).unwrap_or_else(|error| panic!("{error}: {}", raw.data))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_web_tool_the_host_never_answers_is_failed_at_its_deadline() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(browser_action_reply()),
        Script::Immediate(support::wire::text_reply("宿主没响应，我改用文字说明。")),
    ]);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = ToolTableSpec::Standard;
    template.remote_tool_timeout = Some(Duration::from_millis(300));
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |config| config,
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
        Some("{\"text\":\"展示卡片\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);

    // 这一段**故意什么都不做**：没有 `POST /tool_result`，没有 `POST /cancel`。
    let mut saw_timeout = false;
    let ending = loop {
        let frame = next_frame(&mut sse);
        saw_timeout |= matches!(&frame.event, SessionEvent::ToolExecuted { tool, is_error: true, .. } if &**tool == "browser_action");
        if let SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) = &frame.event
            && status.is_terminal()
        {
            break format!("{status:?}");
        }
    };

    // **先分清这一轮是怎么结束的，再谈截止线。** 机器负载高的时候本测试假红过一次
    // （整轮 0.05s 就到终态、`saw_timeout` 为假）：真实原因是第一次 provider 调用
    // 自己没成功、turn 直接 `Failed`，跟 060 的截止线毫无关系。下面那条断言原本会
    // 一口咬定「截止线没生效」，把人指到错的地方去改——**改宽截止线正好把护栏拆了**。
    // 所以先按结束方式分流：`Failed` 是环境问题，`Done` 才轮到截止线的账。
    assert!(
        !ending.contains("Failed"),
        "这一轮是 {ending} 结束的，不是正常答完——第一次 provider 调用就没成功，\
         跟 060 的截止线无关（本条在高负载下出现过）。别去动 remote_tool_timeout。",
    );
    assert!(
        saw_timeout,
        "远端调用该在截止线上被判失败（is_error），而不是无声无息地永远等下去（本轮 {ending} 结束）",
    );
    assert_eq!(
        upstream.request_count(),
        2,
        "超时结果该像真回传一样触发同一轮的第二次 provider 调用"
    );
}
