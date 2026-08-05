//! 085：图片只在 HTTP 边界上传，成功后是 `ms://` 引用才会进入 actor/store。

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
async fn text_stays_on_the_old_wire_shape_and_uploaded_reference_survives_recovery() {
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
        r#"{"text":"看收据","images":[{"name":"receipt.png","mime":"image/png","bytes":[137,80,78,71]}]}"#,
    )
    .await;
    let image_turn = request(&upstream, 1);
    assert_eq!(
        last_user_content(&image_turn),
        &json!([
            {"type":"text","text":"看收据"},
            {"type":"image_url","image_url":{"url":"ms://uploaded-image"}}
        ]),
        "上传成功后只有完整 ms:// 引用能跨过 actor 边界"
    );
    assert_eq!(upstream.upload_count(), 1, "一张附件只允许上传一次");
    let upload = upstream
        .calls()
        .into_iter()
        .find(|call| call.path.ends_with("/files"))
        .expect("图片输入必须有一次上传请求");
    assert_eq!(
        upload.path, "/v1/files",
        "上传必须走独立的文件端点，不能把聊天路径继续追加 /files"
    );
    assert!(
        upload.body.contains("name=\"purpose\"\r\n\r\nimage")
            && upload.body.contains("name=\"file\""),
        "上传必须保留 purpose=image 和 file multipart 字段：{}",
        upload.body
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
    turn(second_addr, "image-store", r#"{"text":"继续"}"#).await;
    let recovered = request(&upstream, 2);
    assert!(
        recovered["messages"]
            .as_array()
            .expect("Kimi 请求必须有 messages")
            .iter()
            .any(|message| message["content"]
                == json!([
                    {"type":"text","text":"看收据"},
                    {"type":"image_url","image_url":{"url":"ms://uploaded-image"}}
                ])),
        "恢复后的 store 历史必须仍有完整图片块：{recovered}"
    );
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
