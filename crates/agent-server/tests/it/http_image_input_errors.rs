//! 093：图片入口先受理，请求期上传失败再通过 actor 终态报告。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::{ErrorClass, Failure, Notice, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::json;

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client::{self, SseReader};

#[tokio::test(flavor = "multi_thread")]
async fn rejected_upload_is_accepted_at_ingress_then_fails_the_turn() {
    assert_request_time_upload_failure(500, "image-request-reject", ErrorClass::Retryable, 3).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_upload_is_accepted_at_ingress_then_fails_the_turn() {
    assert_request_time_upload_failure(401, "image-request-auth", ErrorClass::BadRequest, 1).await;
}

async fn assert_request_time_upload_failure(
    status: u16,
    dir_name: &str,
    expected_class: ErrorClass,
    expected_uploads: usize,
) {
    let upstream = ImageUploadUpstream::start(UploadReply::Status(status));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir(dir_name),
    ))
    .await;
    create(addr, "failed-image");

    let (_, _, mut sse) = http_client::connect_sse(addr, "/sessions/failed-image/events", None);
    let accepted = http_client::request(
        addr,
        "POST",
        "/sessions/failed-image/input",
        Some(
            r#"{"text":"请看图","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        ),
    );
    assert_eq!(
        accepted.status, 202,
        "入口只登记 attachment，不同步等待 /files：{}",
        accepted.body
    );
    assert_eq!(
        wait_for_terminal(&mut sse),
        TurnStatus::Failed(Failure::Provider(expected_class)),
        "上传 HTTP {status} 必须按稳定错误契约进入 provider 失败终态"
    );
    assert_eq!(
        upstream.upload_count(),
        expected_uploads,
        "上传 HTTP {status} 的重试次数必须服从错误分类"
    );
    assert_eq!(upstream.chat_count(), 0, "上传失败后不得发起 chat");
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
    panic!("等待上传失败终态超时");
}
