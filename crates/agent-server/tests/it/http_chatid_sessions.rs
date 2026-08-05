//! 055：业务 chatid 作为会话身份时，重复请求要接上活会话，关闭后的同一
//! chatid 要从默认 jsonl 恢复；不可信 id 则在任何文件系统副作用之前拒绝。

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use agent_core::Notice;
use agent_server::{AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle};

use support::http_client::{self, SseReader};
use support::server::{FakeServer, Script};
use support::wire::text_reply;

const CHAT_ID: &str = "customer_42-chat";

#[tokio::test(flavor = "multi_thread")]
async fn repeated_chatid_reattaches_to_the_live_session_without_clearing_history() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("first reply"))]);
    let (addr, sessions) = start(persistent_template(upstream.endpoint(), support::temp_dir("chatid-live"))).await;

    let first = create_with_chatid(addr, CHAT_ID);
    assert_eq!(first.status, 201, "{}", first.body);
    assert_eq!(support::extract_json_string_field(&first.body, "id"), CHAT_ID);
    assert_eq!(support::extract_json_string_field(&first.body, "outcome"), "created");

    let (_, _, mut sse) = http_client::connect_sse(addr, &format!("/sessions/{CHAT_ID}/events"), None);
    post_input(addr, CHAT_ID, "remember-on-live-session");
    wait_for_terminal(&mut sse);

    let repeated = create_with_chatid(addr, CHAT_ID);
    assert_eq!(repeated.status, 200, "{}", repeated.body);
    assert_eq!(support::extract_json_string_field(&repeated.body, "id"), CHAT_ID);
    assert_eq!(support::extract_json_string_field(&repeated.body, "outcome"), "existing");

    post_input(addr, CHAT_ID, "the-next-message");
    wait_for_terminal(&mut sse);
    let request_bodies = upstream.bodies();
    assert_eq!(request_bodies.len(), 2, "两轮都该到达上游：{request_bodies:?}");
    assert!(request_bodies[1].contains("remember-on-live-session"), "重复 POST 不能清空已有历史：{}", request_bodies[1]);

    assert!(sessions.close_all().iter().all(|(_, result)| result.is_ok()));
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_chatid_recovers_history_from_its_default_session_file() {
    let upstream = FakeServer::start(vec![Script::Immediate(text_reply("persisted reply"))]);
    let sessions_dir = support::temp_dir("chatid-recovered");
    let (first_addr, first_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir.clone())).await;

    assert_eq!(create_with_chatid(first_addr, CHAT_ID).status, 201);
    let (_, _, mut first_sse) = http_client::connect_sse(first_addr, &format!("/sessions/{CHAT_ID}/events"), None);
    post_input(first_addr, CHAT_ID, "remember-after-restart");
    wait_for_terminal(&mut first_sse);
    assert!(first_sessions.close_all().iter().all(|(_, result)| result.is_ok()));
    assert!(sessions_dir.join(format!("{CHAT_ID}.jsonl")).is_file(), "优雅关闭后恢复源文件该已完整落盘");

    let (second_addr, second_sessions) = start(persistent_template(upstream.endpoint(), sessions_dir)).await;
    let recovered = create_with_chatid(second_addr, CHAT_ID);
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(support::extract_json_string_field(&recovered.body, "outcome"), "recovered");

    let (_, _, mut recovered_sse) = http_client::connect_sse(second_addr, &format!("/sessions/{CHAT_ID}/events"), None);
    post_input(second_addr, CHAT_ID, "new-after-restart");
    wait_for_terminal(&mut recovered_sse);
    let request_bodies = upstream.bodies();
    assert_eq!(request_bodies.len(), 2, "恢复后第二轮也该调用上游：{request_bodies:?}");
    assert!(request_bodies[1].contains("remember-after-restart"), "恢复出的会话该携带关闭前的内容：{}", request_bodies[1]);

    assert!(second_sessions.close_all().iter().all(|(_, result)| result.is_ok()));
}

/// 状态码只是表象，这条测试真正要钉的是**文件系统一个字节都没被写**。
///
/// `sessions-dir` 和 `tools-root` 都放进私有沙箱的第三层（`sandbox/a/b/`）：
/// `../../` 这种穿越即使成功也仍然落在沙箱之内，于是「沙箱整棵树逐项不变」
/// 这一条断言就同时盖住了「目录内部被污染」和「向上逃逸」。反过来，直接写
/// `assert!(!tools_root.join(id).exists())` 是错的——`/tmp/x/../../etc/passwd`
/// 在 `TMPDIR=/tmp` 的机器上会正规化成真实存在的 `/etc/passwd`，那条断言会
/// 在没有任何 bug 时假失败。
#[tokio::test(flavor = "multi_thread")]
async fn invalid_chatids_are_rejected_before_creating_any_session_file() {
    let upstream = FakeServer::start(vec![]);
    let sandbox = support::temp_dir("chatid-invalid");
    let sessions_dir = sandbox.join("a/b/sessions");
    let tools_root = sandbox.join("a/b/tools");
    std::fs::create_dir_all(&sessions_dir).unwrap();
    std::fs::create_dir_all(&tools_root).unwrap();
    let mut template = persistent_template(upstream.endpoint(), sessions_dir);
    template.tools_root = tools_root;
    let (addr, sessions) = start(template).await;
    let untouched = tree_under(&sandbox);
    let too_long = "a".repeat(129);

    for id in ["../../etc/passwd", "a/b", "..", "", "customer.id", "客户", &too_long] {
        let response = create_with_chatid(addr, id);
        assert_eq!(response.status, 400, "id={id:?}, body={}", response.body);
        assert!(response.body.contains("\"bad_request\""), "{}", response.body);
        assert_eq!(tree_under(&sandbox), untouched, "无效 id 不得在文件系统上留下任何痕迹：id={id:?}");
    }
    assert!(sessions.ids().is_empty(), "无效请求不得登记 session");
}

#[tokio::test(flavor = "multi_thread")]
async fn omitting_chatid_keeps_the_legacy_generated_id_and_created_status() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(support::http_server::session_template(upstream.endpoint())).await;

    let response = http_client::request(addr, "POST", "/sessions", Some("{}"));
    assert_eq!(response.status, 201, "{}", response.body);
    assert!(support::extract_json_string_field(&response.body, "id").starts_with("sess-"), "{}", response.body);
    assert!(!response.body.contains("\"outcome\""), "旧请求的响应形状保持只含 id：{}", response.body);

    assert!(sessions.close_all().iter().all(|(_, result)| result.is_ok()));
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(ServerConfig::new(template));
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn persistent_template(endpoint: String, sessions_dir: PathBuf) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.default_sessions_dir = Some(sessions_dir);
    template
}

/// `root` 下每一个后代的相对路径，排序后可直接相等比较。
fn tree_under(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("沙箱目录该读得动") {
            let path = entry.expect("目录项该读得动").path();
            found.push(path.strip_prefix(root).expect("后代必然以 root 打头").display().to_string());
            if path.is_dir() {
                stack.push(path);
            }
        }
    }
    found.sort();
    found
}

fn create_with_chatid(addr: SocketAddr, id: &str) -> support::http_client::HttpResponse {
    let body = serde_json::json!({ "id": id }).to_string();
    http_client::request(addr, "POST", "/sessions", Some(&body))
}

fn post_input(addr: SocketAddr, id: &str, text: &str) {
    let body = serde_json::json!({ "text": text }).to_string();
    let response = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some(&body));
    assert_eq!(response.status, 202, "{}", response.body);
}

fn wait_for_terminal(sse: &mut SseReader) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let Some(event) = sse.next_event(remaining) else { break };
        let frame: Frame = serde_json::from_str(&event.data).unwrap_or_else(|error| panic!("SSE 帧不是 Frame：{error}: {}", event.data));
        if matches!(frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()) {
            return;
        }
    }
    panic!("等待回合终态超时");
}
