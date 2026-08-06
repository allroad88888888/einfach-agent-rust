//! 093：上传在 session actor 的请求期 IO 线程执行，不堵入口或其他 actor。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::{Notice, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::json;

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client::{self, SseReader};

#[tokio::test(flavor = "multi_thread")]
async fn a_slow_upload_does_not_hold_another_session_actor() {
    let upstream = ImageUploadUpstream::start(UploadReply::SlowOk(Duration::from_millis(700)));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-request-parallel"),
    ))
    .await;
    create(addr, "slow-image");
    create(addr, "plain-during-upload");

    let (_, _, mut image_sse) = http_client::connect_sse(addr, "/sessions/slow-image/events", None);
    let image_response = post_image(addr, "slow-image");
    assert_eq!(image_response.status, 202, "图片入口必须立即受理");
    wait_until_upload_started(&upstream);

    let plain_started = Instant::now();
    assert!(matches!(
        turn(addr, "plain-during-upload", r#"{"text":"不等图片"}"#).await,
        TurnStatus::Done { .. }
    ));
    assert!(
        plain_started.elapsed() < Duration::from_millis(500),
        "另一会话不应等待正在执行的慢 /files 请求"
    );
    assert!(matches!(
        wait_for_terminal(&mut image_sse),
        TurnStatus::Done { .. }
    ));
    assert_eq!(upstream.upload_count(), 1, "图片仅应上传一次");
    assert_eq!(upstream.chat_count(), 2, "两个会话都应完成 chat");
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn attachment_ingress_does_not_wait_for_request_time_upload() {
    let upstream = ImageUploadUpstream::start(UploadReply::SlowOk(Duration::from_millis(700)));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-request-own-actor"),
    ))
    .await;
    create(addr, "shared-actor");
    let (_, _, mut sse) = http_client::connect_sse(addr, "/sessions/shared-actor/events", None);

    let started = Instant::now();
    let image_response = post_image(addr, "shared-actor");
    let ingress_elapsed = started.elapsed();
    assert!(
        ingress_elapsed < Duration::from_millis(500),
        "attachment 登记不应等待 700ms 的 /files 端点：{ingress_elapsed:?}"
    );
    assert_eq!(image_response.status, 202, "图片入口必须立即受理");
    assert!(matches!(
        wait_for_terminal(&mut sse),
        TurnStatus::Done { .. }
    ));
    assert!(
        started.elapsed() >= Duration::from_millis(650),
        "轮终态必须等待请求期上传完成"
    );
    assert_eq!(upstream.upload_count(), 1);
    assert_eq!(upstream.chat_count(), 1);
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

fn post_image(addr: SocketAddr, id: &str) -> http_client::HttpResponse {
    http_client::request(
        addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some(
            r#"{"text":"慢上传","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        ),
    )
}

async fn turn(addr: SocketAddr, id: &str, body: &str) -> TurnStatus {
    let (_, _, mut sse) = http_client::connect_sse(addr, &format!("/sessions/{id}/events"), None);
    let response = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some(body));
    assert_eq!(
        response.status, 202,
        "输入应被 actor 接收：{}",
        response.body
    );
    wait_for_terminal(&mut sse)
}

fn wait_until_upload_started(upstream: &ImageUploadUpstream) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if upstream.upload_count() > 0 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("等待慢上传开始超时");
}

fn wait_for_terminal(sse: &mut SseReader) -> TurnStatus {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|error| panic!("SSE 不是 Frame：{error}: {}", event.data));
        if let SessionEvent::Notice(Notice::TurnStatusChanged { status }) = frame.event
            && status.is_terminal()
        {
            return status;
        }
    }
    panic!("等待输入轮终态超时");
}
