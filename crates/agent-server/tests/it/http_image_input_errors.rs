//! 085：图片输入不在 HTTP 边界上传；上游 /files 状态不得影响会话受理。

use crate::support;
use std::net::SocketAddr;
use std::time::Duration;

use agent_providers::kimi::Kimi;
use agent_server::{AgentServer, ServerConfig, SessionTemplate, SessionsHandle};
use serde_json::json;

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client;

#[tokio::test(flavor = "multi_thread")]
async fn rejected_upload_endpoint_does_not_block_attachment_ingress() {
    let upstream = ImageUploadUpstream::start(UploadReply::Status(500));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-reject"),
    ))
    .await;
    create(addr, "reject-image");

    let accepted = http_client::request(
        addr,
        "POST",
        "/sessions/reject-image/input",
        Some(
            r#"{"text":"不能留下","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        ),
    );
    assert_eq!(
        accepted.status, 202,
        "输入只登记 attachment 引用，不能因未调用的 /files 返回 500 而拒绝：{}",
        accepted.body
    );
    assert!(
        upstream.upload_count() == 0,
        "入口不应触碰 /files，即使该端点会返回 500"
    );

    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unauthorized_upload_endpoint_does_not_block_attachment_ingress() {
    let upstream = ImageUploadUpstream::start(UploadReply::Status(401));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-auth"),
    ))
    .await;
    create(addr, "unauthorized-image");

    let accepted = http_client::request(
        addr,
        "POST",
        "/sessions/unauthorized-image/input",
        Some(
            r#"{"text":"密钥错误","images":[{"mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        ),
    );
    assert_eq!(
        accepted.status, 202,
        "输入只登记 attachment 引用，不能因未调用的 /files 返回 401 而拒绝：{}",
        accepted.body
    );
    assert!(
        upstream.upload_count() == 0,
        "入口不应触碰 /files，即使该端点会返回 401"
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
