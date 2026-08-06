//! 085：图片入口不等待 /files，慢配置端点不能阻塞 session actor。

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
async fn a_slow_upload_endpoint_does_not_hold_another_session_actor() {
    let upstream = ImageUploadUpstream::start(UploadReply::SlowOk(Duration::from_millis(700)));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-parallel"),
    ))
    .await;
    create(addr, "slow-image");
    create(addr, "plain-during-upload");

    let image_addr = addr;
    let image_post = tokio::task::spawn_blocking(move || {
        http_client::request(
            image_addr,
            "POST",
            "/sessions/slow-image/input",
            Some(
                r#"{"text":"慢上传","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
            ),
        )
    });
    let started = Instant::now();
    turn(addr, "plain-during-upload", r#"{"text":"不等图片"}"#).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "另一会话不应等待未被调用的慢 /files 端点"
    );
    let image_response = image_post.await.expect("图片 POST 线程");
    assert_eq!(
        image_response.status, 202,
        "图片入口只登记 attachment 引用，应立即受理：{}",
        image_response.body
    );
    assert_eq!(upstream.upload_count(), 0, "入口不应请求慢 /files 端点");
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_ingress_does_not_wait_for_a_slow_upload_endpoint() {
    let upstream = ImageUploadUpstream::start(UploadReply::SlowOk(Duration::from_millis(700)));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-own-actor"),
    ))
    .await;
    create(addr, "shared-actor");

    let started = Instant::now();
    let image_response = http_client::request(
        addr,
        "POST",
        "/sessions/shared-actor/input",
        Some(
            r#"{"text":"慢上传","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        ),
    );
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "attachment 登记不应等待 700ms 的 /files 端点"
    );
    assert_eq!(image_response.status, 202, "图片入口必须立即受理");
    assert_eq!(upstream.upload_count(), 0, "入口不应请求慢 /files 端点");
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
