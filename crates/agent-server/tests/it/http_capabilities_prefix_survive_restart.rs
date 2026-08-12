//! 156 的核心验收，重启部分（跨进程形态，手法照抄
//! `http_capabilities_survive_restart.rs` 对 `capabilities.tools` 的同款）：
//! 宿主声明的开局块（M17，决策 31）**进 store**，会话关掉再按同一个 chatid 打开
//! 时**不带任何 `capabilities` 也原样回来**，而且首轮请求体与关闭前逐字节一致
//! （134 的状态回放，不重跑 `run_session_start`）。
//!
//! spawn 的 `inherit_prefix` 在恢复后收/拒两路的验收在姊妹文件
//! `http_capabilities_prefix_inherit_after_restart.rs`（职责分开：这份管
//! 「重启这一步本身」，那份管「重启之后 spawn 认不认这份复原出的表」）。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::{FakeServer, Script};
use crate::support::wire::text_reply;

const CHAT_ID: &str = "prefix-restart-chat";
const PREFIX_A_NAME: &str = "web:crm/briefing";
const PREFIX_A_TEXT: &str = "PREFIX-RESTART-A-2c91 今天的客户上下文";
const PREFIX_B_NAME: &str = "desk:ops/standup";
const PREFIX_B_TEXT: &str = "PREFIX-RESTART-B-77ae 今天的运维简报";

fn declaration() -> Value {
    json!({
        "prefix": [
            { "name": PREFIX_A_NAME, "text": PREFIX_A_TEXT },
            { "name": PREFIX_B_NAME, "text": PREFIX_B_TEXT }
        ]
    })
}

/// **本 issue 的全部意义**：建会话 + 声明开局块 → 一轮 → 关掉 → 同 chatid 重开、
/// **不带任何 `capabilities`** → 首轮请求体的 system 段仍有两块，而且与关闭前
/// **逐字节相同**（红线 11：不是「块还在」，是「一个字节都没变」，前缀缓存才接
/// 得上）。
#[tokio::test(flavor = "multi_thread")]
async fn a_recovered_session_brings_its_declared_prefix_back_without_being_told_again() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("prefix-restart");

    // ── 第一次：带声明建会话，跑一轮。
    let (first_addr, first_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    let created = create(
        first_addr,
        json!({ "id": CHAT_ID, "capabilities": declaration() }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let before = one_turn(&upstream, first_addr).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    // ── 两块都真的在（正对照：光有下面的字节比较，一个「重启后前缀整段消失，
    //    但两次都消失」的实现也会通过字节相等）。
    let before_text = system_text(&before);
    assert!(
        before_text.contains(PREFIX_A_TEXT) && before_text.contains(PREFIX_B_TEXT),
        "声明的两块该真的进了 system 段：{before_text}"
    );

    // ── 第二次：**请求体里一个 `capabilities` 字都没有**。
    let (second_addr, second_sessions) =
        start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    let recovered = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(
        support::extract_json_string_field(&recovered.body, "outcome"),
        "recovered"
    );
    let after = one_turn(&upstream, second_addr).await;

    // **不比较整个 `messages`**：`after` 这一轮是在一份已经有一轮历史的会话上
    // 跑的（`before` 那轮的 user/assistant 两条消息现在是历史），`messages`
    // 数组自然比 `before` 多——这是正常的对话增长，不是本条要看的事。本条要
    // 证的是「system 段（前缀块的落点）与 tools 段没有因为恢复而重新计算出
    // 不同的字节」，两段各自跟静态部署配置/开局块一一对应，天然不随对话轮数
    // 变化，比较它们才是红线 11 的真意。
    assert_eq!(
        system_text(&after),
        system_text(&before),
        "恢复后 system 段必须与关闭前逐字节相同（回放不重跑 run_session_start）"
    );
    assert_eq!(
        serde_json::to_string(&after["tools"]).unwrap(),
        serde_json::to_string(&before["tools"]).unwrap(),
        "恢复后 tools 段同样该逐字节相同"
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

/// 有历史 + 再声明（这次只带 `prefix`，不带 `tools`/`skills`）→ 400
/// `session_has_history`——验证既有闸（073）自动罩住这个新字段，**不写新逻辑**
/// （156 §做什么 第 4 条）。手法照抄 `http_capabilities_survive_restart.rs::
/// declaring_again_on_a_session_with_history_is_refused_with_its_own_error_code`：
/// 必须先关掉第一个 server 实例，`open_or_get_with` 的「只有赢家查历史」那道
/// 短路才会真的走到查文件是否存在这一步。
#[tokio::test(flavor = "multi_thread")]
async fn declaring_prefix_again_on_a_session_with_history_is_refused() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("好的。"))]);
    let sessions_dir = support::temp_dir("prefix-restart-refuse");

    let (first_addr, first_sessions) = start(persistent_template(
        upstream.endpoint(),
        sessions_dir.clone(),
    ))
    .await;
    assert_eq!(
        create(
            first_addr,
            json!({ "id": CHAT_ID, "capabilities": declaration() })
        )
        .status,
        201
    );
    one_turn(&upstream, first_addr).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    let (addr, sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;

    let refused = create(
        addr,
        json!({ "id": CHAT_ID, "capabilities": { "prefix": [ { "name": PREFIX_A_NAME, "text": "重新声明一遍" } ] } }),
    );
    assert_eq!(refused.status, 400, "{}", refused.body);
    assert!(
        refused.body.contains("\"session_has_history\""),
        "只带 prefix 的重新声明也该撞这道闸，不能是通用 bad_request：{}",
        refused.body
    );
    assert!(
        sessions.ids().is_empty(),
        "被拒的请求不该把会话 open 起来：{:?}",
        sessions.ids()
    );

    // 不带声明：照常恢复，前缀块还在（拒绝没有把会话弄坏）。
    let ok = create(addr, json!({ "id": CHAT_ID }));
    assert_eq!(ok.status, 200, "{}", ok.body);
    assert!(system_text(&one_turn(&upstream, addr).await).contains(PREFIX_A_TEXT));
    assert!(sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

async fn start(template: agent_server::SessionTemplate) -> (SocketAddr, agent_server::SessionsHandle) {
    let server = agent_server::AgentServer::new(
        agent_server::ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn persistent_template(endpoint: String, sessions_dir: PathBuf) -> agent_server::SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.default_sessions_dir = Some(sessions_dir);
    template
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}

/// 发一句话、等这一轮走到终态，把这次 provider 调用请求体取出来。
async fn one_turn(upstream: &FakeServer, addr: SocketAddr) -> Value {
    let before = upstream.request_count();
    let (_, _, mut sse) =
        http_client::connect_sse(addr, &format!("/sessions/{CHAT_ID}/events"), None);
    let input = http_client::request(
        addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(input.status, 202, "{}", input.body);
    wait_for_terminal(&mut sse);

    let raw = upstream.bodies().swap_remove(before);
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("请求体不是 JSON：{e}\n{raw}"))
}

fn wait_for_terminal(sse: &mut http_client::SseReader) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(event) = sse.next_event(remaining) else {
            break;
        };
        let frame: agent_server::Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|e| panic!("SSE 帧不是 Frame：{e}: {}", event.data));
        if matches!(
            frame.event,
            agent_server::SessionEvent::Notice(agent_core::Notice::TurnStatusChanged { status })
                if status.is_terminal()
        ) {
            return;
        }
    }
    panic!("等待回合终态超时");
}

/// 请求体里那条 `role: "system"` 消息的正文。
fn system_text(body: &Value) -> String {
    body["messages"]
        .as_array()
        .expect("请求体里该有 messages")
        .iter()
        .find(|m| m["role"] == json!("system"))
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .unwrap_or_default()
}
