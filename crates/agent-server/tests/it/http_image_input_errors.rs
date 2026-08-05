//! 085：图片上传失败必须停在 HTTP 边界，且错误不泄露密钥。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::Notice;
use agent_providers::kimi::Kimi;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::json;

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client;

#[tokio::test(flavor = "multi_thread")]
async fn rejected_upload_leaves_no_history_and_error_kinds_are_readable_without_the_key() {
    let upstream = ImageUploadUpstream::start(UploadReply::Status(500));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-reject"),
    ))
    .await;
    create(addr, "reject-image");

    let rejected = http_client::request(
        addr,
        "POST",
        "/sessions/reject-image/input",
        Some(r#"{"text":"不能留下","images":[{"mime":"image/png","bytes":[1]}]}"#),
    );
    assert_eq!(
        rejected.status, 400,
        "上传 500 必须在 HTTP 边界变成 400：{}",
        rejected.body
    );
    assert!(
        rejected.body.contains("HTTP 500"),
        "provider 拒绝必须可辨识：{}",
        rejected.body
    );
    assert!(
        !rejected.body.contains("test-api-key"),
        "错误报文不得泄露 api key：{}",
        rejected.body
    );
    assert!(
        upstream.chat_bodies().is_empty(),
        "上传失败不能 dispatch，不能写进会话历史"
    );

    turn(addr, "reject-image", r#"{"text":"之后正常"}"#).await;
    assert!(
        upstream.chat_bodies()[0].contains("之后正常"),
        "失败附件后下一句纯文本必须是唯一的历史输入"
    );
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_upload_is_distinct_from_provider_rejection_and_redacts_the_key() {
    let upstream = ImageUploadUpstream::start(UploadReply::Status(401));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-auth"),
    ))
    .await;
    create(addr, "unauthorized-image");

    let unauthorized = http_client::request(
        addr,
        "POST",
        "/sessions/unauthorized-image/input",
        Some(r#"{"text":"密钥错误","images":[{"mime":"image/png","bytes":[1]}]}"#),
    );
    assert_eq!(
        unauthorized.status, 400,
        "401 上传错误也必须停在 HTTP 边界：{}",
        unauthorized.body
    );
    assert!(
        unauthorized.body.contains("认证失败"),
        "401 必须和 provider 拒绝区分：{}",
        unauthorized.body
    );
    assert!(
        !unauthorized.body.contains("test-api-key"),
        "401 错误不得泄露 api key：{}",
        unauthorized.body
    );
    assert!(
        upstream.chat_bodies().is_empty(),
        "认证失败同样不能进入 actor"
    );
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

async fn start(template: SessionTemplate) -> (SocketAddr, SessionsHandle) {
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server
        .bind("127.0.0.1:0".parse().expect("loopback 地址"))
        .await
        .expect("绑定 HTTP 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;
    (addr, sessions)
}

fn template(
    endpoint: String,
    upload_base_url: String,
    sessions_dir: std::path::PathBuf,
) -> SessionTemplate {
    let mut template = support::http_server::session_template(endpoint);
    template.upload_base_url = upload_base_url;
    template.provider = std::sync::Arc::new(Kimi);
    template.model = std::sync::Arc::from("kimi-for-image-test");
    template.api_key = "test-api-key".to_string();
    template.default_sessions_dir = Some(sessions_dir);
    template
}

fn create(addr: SocketAddr, id: &str) {
    let created = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({ "id": id }).to_string()),
    );
    assert_eq!(created.status, 201, "创建 {id} 失败：{}", created.body);
}

async fn turn(addr: SocketAddr, id: &str, body: &str) {
    let (_, _, mut sse) = http_client::connect_sse(addr, &format!("/sessions/{id}/events"), None);
    let response = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some(body));
    assert_eq!(
        response.status, 202,
        "输入应被 actor 接收：{}",
        response.body
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|error| panic!("SSE 不是 Frame：{error}: {}", event.data));
        if matches!(frame.event, SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal())
        {
            return;
        }
    }
    panic!("等待输入轮终态超时");
}
