//! 独立测试 agent 依据 156 + HOST-CAPABILITIES §三/§八之三 写的规格测试——不看
//! `http/capabilities/{validate_prefix,capability_prefix,assemble}.rs`/
//! `actor/capabilities.rs` 的实现，只按协议契约推断预期。
//!
//! 本文件管**声明与 undo 的边界**这一件事：`capabilities.prefix` 声明是**建会话
//! 时**写进 store 的会话状态（HOST-CAPABILITIES §三：「声明是会话状态,不是部署
//! 配置」），跟对话轮次不是同一层——`/undo` 撤的是「上一轮」，不该连带撤掉建
//! 会话时就落好的前缀块。这是文档没有明写、但从「恢复 = 原模原样复刻」同一条
//! 哲学能推出来的预期：undo/redo/崩溃恢复是同一套机制的四个投影,前缀声明既然
//! 在恢复路径上原样复刻（073/135 同构),在 undo 路径上也不该无端消失。

use crate::support;
use std::time::Duration;

use agent_core::Notice;
use agent_server::{Frame, SessionEvent, UndoOutcome};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

const CHAT_ID: &str = "prefix-decl-undo-indep";
const MARKER: &str = "PREFIX_UNDO_MARKER_7QX2";

/// 撤销一轮对话之后，建会话时声明的前缀块该**原样还在**——undo 撤的是轮次
/// 状态，不是会话创建时落的声明。两个turn 之间的 system 段甚至该逐字节相同
/// （跟「恢复后首轮 system 逐字节不变」同一条红线 11 精神：声明只在建会话时
/// 写一次，往后不该因为任何轮内操作而变化）。
#[tokio::test(flavor = "multi_thread")]
async fn undoing_a_turn_leaves_the_declared_prefix_block_byte_identical() {
    let upstream = FakeServer::start(vec![
        Script::Immediate(text_reply("第一轮回复。")),
        Script::Immediate(text_reply("第二轮回复。")),
    ]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let created = create(
        server.addr,
        json!({
            "id": CHAT_ID,
            "capabilities": { "prefix": [ { "name": "web:crm/briefing", "text": MARKER } ] }
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);

    let (status, _, mut sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{CHAT_ID}/events"), None);
    assert_eq!(status, 200);

    // ── 第一轮，跑到终态。
    run_turn(server.addr, &mut sse).await;
    let first = latest_body(&upstream);
    assert!(
        system_text(&first).contains(MARKER),
        "第一轮 system 段该含声明的前缀文本：{}",
        system_text(&first)
    );

    // ── 撤销这一轮。
    let undo = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/undo"),
        Some("{}"),
    );
    assert_eq!(undo.status, 202, "{}", undo.body);
    let frame = next_typed(&mut sse, Duration::from_secs(3));
    assert!(
        matches!(frame.event, SessionEvent::Undo(UndoOutcome::Applied { .. })),
        "该收到 undo 生效的通知：{frame:?}"
    );

    // ── 再跑一轮：前缀块该原样还在，且与撤销前那一轮的 system 段逐字节相同
    //    ——它只在建会话时写一次，undo 一轮对话不该碰到它。
    run_turn(server.addr, &mut sse).await;
    let after_undo = latest_body(&upstream);
    assert!(
        system_text(&after_undo).contains(MARKER),
        "undo 一轮之后，前缀块不该被一并撤掉：{}",
        system_text(&after_undo)
    );
    assert_eq!(
        system_text(&after_undo),
        system_text(&first),
        "声明的前缀块只在建会话时写一次，undo 一轮对话前后 system 段该逐字节相同"
    );
}

async fn run_turn(addr: std::net::SocketAddr, sse: &mut http_client::SseReader) {
    let input = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(input.status, 202, "{}", input.body);
    loop {
        let frame = next_typed(sse, Duration::from_secs(5));
        if matches!(&frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal())
        {
            return;
        }
    }
}

/// 跳过 `AgentTree` 快照帧——跟 `http_undo_redo_cancel_endpoints.rs` 同一个理由：
/// 这个文件测的是 undo 对前缀块的影响，树帧是噪声。
fn next_typed(sse: &mut http_client::SseReader, budget: Duration) -> Frame {
    loop {
        let frame = sse.next_event(budget).expect("该收到一帧");
        let parsed: Frame =
            serde_json::from_str(&frame.data).unwrap_or_else(|e| panic!("{e}: {}", frame.data));
        if matches!(parsed.event, SessionEvent::AgentTree(_)) {
            continue;
        }
        return parsed;
    }
}

fn create(addr: std::net::SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

fn latest_body(upstream: &FakeServer) -> Value {
    let bodies = upstream.bodies();
    let raw = bodies.last().expect("该至少有一次 provider 调用");
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{raw}"))
}

fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("system"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}
