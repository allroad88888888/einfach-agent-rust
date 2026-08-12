//! 独立测试 agent 依据 156 + HOST-CAPABILITIES §三/§八之三 写的规格测试——不看
//! `http/capabilities/{validate_prefix,capability_prefix,assemble}.rs`/
//! `actor/capabilities.rs` 的实现，只按协议契约断言。
//!
//! 本文件管两件事，都跟 `name` 本身的形状无关（那部分见
//! `prefix_decl_name_reject_indep.rs`）：`text` 为空该 400；以及两种 400 能不能
//! 被宿主机械区分开。HOST-CAPABILITIES 原句：
//!
//! > 宿主要能把「我工具名写错了」（`bad_request`，改名字重发）和「这会话已有
//! > 历史」（`session_has_history`，去掉声明重发）分开——两者都是 400，正确的
//! > 应对却相反。

use crate::support;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use agent_server::{AgentServer, ServerConfig, SessionsHandle};
use serde_json::{Value, json};

use crate::support::http_client;
use crate::support::server::FakeServer;

const CHAT_ID: &str = "prefix-decl-reject-indep";

/// **校验条 3**：`text` 为空 → 400——本地 timed 工具空文本是「执行结果」语义（135），
/// 声明一段常量空文本只能是笔误（156 判据）。
#[tokio::test(flavor = "multi_thread")]
async fn empty_text_is_400_bad_request() {
    let upstream = FakeServer::start(vec![]);
    let (addr, sessions) = start(&upstream).await;

    let response = create(
        addr,
        json!({
            "id": CHAT_ID,
            "capabilities": {
                "prefix": [ { "name": "web:crm/briefing", "text": "" } ]
            }
        }),
    );
    assert_eq!(response.status, 400, "{}", response.body);
    assert_eq!(
        support::extract_json_string_field(&response.body, "code"),
        "bad_request"
    );
    assert!(
        response.body.contains("web:crm/briefing"),
        "该点名是哪个块的 text 是空的：{}",
        response.body
    );
    assert!(sessions.ids().is_empty());
}

/// **验收第 4 条 + 两种 400 的可判别性**：已有历史的会话再声明 `prefix`
/// （哪怕只带这一个字段）→ 400 `session_has_history`；跟 `bad_request` 必须能
/// 被宿主机械区分——同一个测试里两种 400 都出现，直接比较 `code` 字段不相等，
/// 钉住「宿主能分辨该改名字重发还是该去掉声明重发」这件事本身。
#[tokio::test(flavor = "multi_thread")]
async fn redeclaring_prefix_on_a_session_with_history_is_session_has_history_not_bad_request() {
    let upstream = FakeServer::start(vec![support::server::Script::Immediate(
        support::wire::text_reply("好的。"),
    )]);
    let sessions_dir = support::temp_dir("prefix-decl-reject-history");

    let (first_addr, first_sessions) = start_with_dir(&upstream, sessions_dir.clone()).await;
    let created = create(
        first_addr,
        json!({
            "id": CHAT_ID,
            "capabilities": { "prefix": [ { "name": "web:crm/briefing", "text": "开局文本" } ] }
        }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    let turn = http_client::request(
        first_addr,
        "POST",
        &format!("/sessions/{CHAT_ID}/input"),
        Some(r#"{"text":"你好"}"#),
    );
    assert_eq!(turn.status, 202, "{}", turn.body);
    wait_for_upstream(&upstream, 1).await;
    assert!(first_sessions.close_all().iter().all(|(_, r)| r.is_ok()));

    let (second_addr, second_sessions) = start_with_dir(&upstream, sessions_dir).await;

    // 只带 `prefix`（不带 tools/skills/disable_builtin）也照样被拒——073 那道闸
    // 是对整个 `capabilities` 判断的，不能只挡加法/减法的其中一种字段。
    let redeclared = create(
        second_addr,
        json!({
            "id": CHAT_ID,
            "capabilities": { "prefix": [ { "name": "web:another/thing", "text": "新文本" } ] }
        }),
    );
    assert_eq!(redeclared.status, 400, "{}", redeclared.body);
    let history_code = support::extract_json_string_field(&redeclared.body, "code");
    assert_eq!(
        history_code, "session_has_history",
        "有历史再声明该是可判别的 session_has_history（去掉声明重发），不是 bad_request：{}",
        redeclared.body
    );

    // 正对照：不带 capabilities 的同一次重开该是 200 recovered——拒绝的原因是
    // 「带了声明」，不是「这个 chatid 有历史就一律不给开」。
    let recovered = create(second_addr, json!({ "id": CHAT_ID }));
    assert_eq!(recovered.status, 200, "{}", recovered.body);
    assert_eq!(
        support::extract_json_string_field(&recovered.body, "outcome"),
        "recovered"
    );

    // 两种 400 的错误码必须不同——这是宿主机械区分「改名字重发」vs「去掉声明
    // 重发」的唯一依据。拿一个坏名字在全新会话上重放一遍 bad_request 那条路，
    // 跟上面 history_code 直接比较不相等。
    let bad_name = create(
        second_addr,
        json!({
            "id": "prefix-decl-reject-freshly-bad",
            "capabilities": { "prefix": [ { "name": "srv:not/allowed", "text": "x" } ] }
        }),
    );
    assert_eq!(bad_name.status, 400, "{}", bad_name.body);
    let bad_name_code = support::extract_json_string_field(&bad_name.body, "code");
    assert_eq!(bad_name_code, "bad_request");
    assert_ne!(
        bad_name_code, history_code,
        "两种 400 的错误码必须不同，否则宿主没法机械区分该怎么应对"
    );

    assert!(second_sessions.close_all().iter().all(|(_, r)| r.is_ok()));
}

async fn start(upstream: &FakeServer) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(support::http_server::session_template(upstream.endpoint()))
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

async fn start_with_dir(
    upstream: &FakeServer,
    sessions_dir: PathBuf,
) -> (SocketAddr, SessionsHandle) {
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.default_sessions_dir = Some(sessions_dir);
    let server = AgentServer::new(
        ServerConfig::new(template).with_private_capability(support::http_server::PRIVATE_CAPABILITY),
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

async fn wait_for_upstream(upstream: &FakeServer, want: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while upstream.request_count() < want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "等第 {want} 次 provider 调用超时，实际 {}",
            upstream.request_count()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn create(addr: SocketAddr, body: Value) -> support::http_client::HttpResponse {
    http_client::request(addr, "POST", "/sessions", Some(&body.to_string()))
}
