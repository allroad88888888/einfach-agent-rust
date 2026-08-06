//! 093：视觉 provider 在请求期解析附件；持久层仍只保存内部句柄。

mod output_privacy;

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::{ErrorClass, Failure, Notice, TurnStatus};
use agent_providers::kimi::Kimi;
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::{Value, json};

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client::{self, SseReader};

#[tokio::test(flavor = "multi_thread")]
async fn visual_request_materializes_images_without_persisting_provider_material() {
    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let sessions_dir = support::temp_dir("image-request-materialization");
    let (first_addr, first_sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        sessions_dir.clone(),
    ))
    .await;

    create(first_addr, "plain-input");
    assert!(matches!(
        turn(first_addr, "plain-input", r#"{"text":"hi"}"#).await,
        TurnStatus::Done { .. }
    ));
    let plain = request(&upstream, 0);
    assert_eq!(
        last_user_content(&plain),
        &json!("hi"),
        "不带图时整条 HTTP→actor→provider 路必须保留旧字符串 content"
    );

    create(first_addr, "image-store");
    assert!(matches!(
        turn(
            first_addr,
            "image-store",
            r#"{"text":"看收据","images":[{"name":"receipt.png","mime":"image/png","bytes":[137,80,78,71,13,10,26,10]}]}"#,
        )
        .await,
        TurnStatus::Done { .. }
    ));
    let image_turn = request(&upstream, 1);
    assert_eq!(
        attachment_reference(last_user_content(&image_turn)),
        "ms://uploaded-image",
        "只有发往视觉 provider 的请求应使用上传后引用"
    );
    assert_eq!(upstream.upload_count(), 1, "视觉 chat 前应上传一次");
    assert!(
        first_sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok()),
        "关闭前必须落盘"
    );

    let journal =
        std::fs::read_to_string(sessions_dir.join("image-store.jsonl")).expect("读取图片会话日志");
    assert!(
        journal.contains("attachment://img_"),
        "持久历史必须保留 session-local attachment 句柄：{journal}"
    );
    for provider_material in ["ms://", "uploaded-image", "test-api-key", "/v1/files"] {
        assert!(
            !journal.contains(provider_material),
            "持久历史不得含 provider 临时材料 {provider_material}：{journal}"
        );
    }

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
    let recovered_status = turn(second_addr, "image-store", r#"{"text":"再看一次"}"#).await;
    assert_eq!(
        recovered_status,
        TurnStatus::Failed(Failure::Provider(ErrorClass::BadRequest)),
        "恢复句柄没有跨进程字节，必须在任何上传/chat 前 fail closed"
    );
    assert_eq!(upstream.upload_count(), 1, "不可用的恢复句柄不得重上传");
    assert_eq!(upstream.chat_count(), 2, "不可用的恢复句柄不得发起 chat");
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
