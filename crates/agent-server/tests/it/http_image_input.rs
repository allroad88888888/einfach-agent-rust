//! 093：HTTP 只把图片放进 session vault，actor/store 只保存内部附件句柄。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::Notice;
use agent_providers::kimi::Kimi;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::{Value, json};

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client::{self, SseReader};

#[tokio::test(flavor = "multi_thread")]
async fn text_stays_on_old_wire_shape_and_attachment_reference_survives_recovery() {
    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let sessions_dir = support::temp_dir("image-ingress-store");
    let (first_addr, first_sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        sessions_dir.clone(),
    ))
    .await;

    create(first_addr, "plain-input");
    turn(first_addr, "plain-input", r#"{"text":"hi"}"#).await;
    let plain = request(&upstream, 0);
    assert_eq!(
        last_user_content(&plain),
        &json!("hi"),
        "不带图时整条 HTTP→actor→provider 路必须保留旧字符串 content"
    );

    create(first_addr, "image-store");
    turn(
        first_addr,
        "image-store",
        r#"{"text":"看收据","images":[{"name":"receipt.png","mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
    )
    .await;
    let image_turn = request(&upstream, 1);
    let first_reference = attachment_reference(last_user_content(&image_turn));
    assert!(
        first_reference.starts_with("attachment://img_"),
        "HTTP→actor→provider 只能传内部句柄，不能泄露字节：{first_reference}"
    );
    assert_eq!(upstream.upload_count(), 0, "输入路由不得上传图片");
    assert!(
        upstream
            .calls()
            .iter()
            .all(|call| !call.path.ends_with("/files")),
        "输入路由不得请求 /files：{:?}",
        upstream
            .calls()
            .iter()
            .map(|call| &call.path)
            .collect::<Vec<_>>()
    );
    assert!(
        first_sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok()),
        "关闭前必须落盘"
    );

    let (second_addr, second_sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        sessions_dir,
    ))
    .await;
    let reopened = http_client::request(
        second_addr,
        "POST",
        "/sessions",
        Some(r#"{"id":"image-store"}"#),
    );
    assert_eq!(
        reopened.status, 200,
        "图片会话必须从 store 恢复：{}",
        reopened.body
    );
    assert!(
        reopened.body.contains("recovered"),
        "重开必须是恢复而不是新会话：{}",
        reopened.body
    );
    turn(
        second_addr,
        "image-store",
        r#"{"text":"再看一张","images":[{"name":"second.png","mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
    )
    .await;
    let recovered = request(&upstream, 2);
    let second_reference = attachment_reference(last_user_content(&recovered));
    assert_ne!(
        second_reference, first_reference,
        "恢复后不得重用旧图片句柄"
    );
    assert!(
        recovered["messages"]
            .as_array()
            .expect("Kimi 请求必须有 messages")
            .iter()
            .any(|message| message["content"]
                == json!([
                    {"type":"text","text":"看收据"},
                    {"type":"image_url","image_url":{"url": first_reference}}
                ])),
        "恢复后的 store 历史必须仍有完整图片块：{recovered}"
    );
    assert_eq!(upstream.upload_count(), 0, "恢复后输入同样不得上传");
    assert!(
        second_sessions
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
    wait_for_terminal(&mut sse);
}

fn wait_for_terminal(sse: &mut SseReader) {
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

fn request(upstream: &ImageUploadUpstream, index: usize) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(body) = upstream.chat_bodies().get(index) {
            return serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("第 {index} 个模型请求不是 JSON：{error}\n{body}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "等待第 {index} 个模型请求超时：{:?}",
        upstream
            .calls()
            .iter()
            .map(|call| &call.path)
            .collect::<Vec<_>>()
    );
}

fn last_user_content(body: &Value) -> &Value {
    body["messages"]
        .as_array()
        .expect("Kimi 请求必须有 messages")
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(|message| &message["content"])
        .expect("请求必须有 user 消息")
}

fn attachment_reference(content: &Value) -> String {
    content
        .as_array()
        .and_then(|blocks| blocks.iter().find(|block| block["type"] == "image_url"))
        .and_then(|block| block["image_url"]["url"].as_str())
        .expect("视觉 provider 的 user content 必须带 image_url")
        .to_owned()
}
