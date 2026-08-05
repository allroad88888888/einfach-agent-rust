//! issue 034 验收：「undo 撞屏障 → SSE 出的 blocked 帧含工具名与 call_id」。
//!
//! 一次真实的 `srv:shell/exec` 调用（`ToolTableSpec::WithShell` 开闸）落地成
//! 一条 barrier entry（020 的裁决：shell 判 `Irreversible`），随后 `/undo`
//! 撞上它——`UndoOutcome::Blocked` 该带着 `Session::barrier_info`（034，
//! agent-core 的公共读口）现查出来的工具名/call_id/label，不是甩一个裸的
//! `barrier_seq` 数字给 web 端猜（027 的原则：让人明白自己在确认什么）。

use crate::support;
use std::time::Duration;

use agent_server::{Frame, SessionEvent, UndoOutcome};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};

/// 第一跳：模型声明一次 `srv:shell/exec`（wire 上的函数名是转义过的，
/// `srv:shell/exec` → `srv_3Ashell_2Fexec`，`agent-providers/src/wire/
/// names.rs` 的规则），call_id 固定成 `call_shell_1` 方便断言。
fn tool_call_reply() -> String {
    [
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_shell_1","type":"function","function":{"name":"srv_3Ashell_2Fexec","arguments":"{\"cmd\": \"echo hi\"}"}}]}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
        "",
    ]
    .join("\n\n")
}

fn next_frame(sse: &mut http_client::SseReader, budget: Duration) -> Frame {
    let raw = sse.next_event(budget).expect("该收到一帧");
    serde_json::from_str(&raw.data).unwrap_or_else(|e| panic!("{e}: {}", raw.data))
}

fn drain_until_terminal(sse: &mut http_client::SseReader) {
    loop {
        let frame = next_frame(sse, Duration::from_secs(5));
        if matches!(
            &frame.event,
            SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status }) if status.is_terminal()
        ) {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn undo_blocked_by_a_shell_barrier_carries_the_tool_name_and_call_id_over_sse() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(tool_call_reply()),
        Script::Immediate(support::wire::text_reply("done")),
    ]);

    let mut template = support::http_server::session_template(upstream.endpoint());
    template.tools = agent_server::ToolTableSpec::WithShell;
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
        Some("{\"text\":\"跑个命令\"}"),
    );
    assert_eq!(input.status, 202, "{}", input.body);
    drain_until_terminal(&mut sse); // 这一轮跑完：shell 已经真的执行过、结果落地成一条 barrier entry。

    let undo = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/undo"),
        Some("{}"),
    );
    assert_eq!(undo.status, 202, "{}", undo.body);

    let frame = loop {
        let frame = next_frame(&mut sse, Duration::from_secs(3));
        if matches!(frame.event, SessionEvent::Undo(_)) {
            break frame;
        }
    };

    assert_eq!(
        frame.agent.as_str(),
        "root",
        "/undo 是会话级命令，该标 root"
    );
    let SessionEvent::Undo(UndoOutcome::Blocked {
        entries,
        tool,
        call_id,
        label,
        ..
    }) = frame.event.clone()
    else {
        panic!("该撞上 barrier，落 Blocked：{frame:?}");
    };
    // 屏障是 tool_result 那条 entry；它之后还有一跳（模型收到结果之后的收尾
    // 文本回复），那条 provider_done entry 比屏障新，undo 从最新开始退，先退
    // 掉它（entries=1），再往下一条（屏障本身）就停住了。
    assert_eq!(entries, 1, "屏障之后那条收尾回复该先被退掉");
    assert_eq!(
        tool.as_deref(),
        Some("srv:shell/exec"),
        "该带工具名，不是只有裸的 barrier_seq：{frame:?}"
    );
    assert_eq!(
        call_id.as_deref(),
        Some("call_shell_1"),
        "该带 call_id：{frame:?}"
    );
    assert_eq!(
        label, "tool_result",
        "被撞上的这条 entry 是工具结果落地那一条：{frame:?}"
    );
}
