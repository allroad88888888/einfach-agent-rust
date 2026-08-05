//! 091：非视觉 provider 的图片只保留元数据，不能在 HTTP 边界提前上传。

use crate::support;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use agent_core::{Adjustment, Notice};
use agent_providers::{Provider, deepseek::DeepSeek};
use agent_server::{
    AgentServer, Frame, ServerConfig, SessionEvent, SessionTemplate, SessionsHandle,
};
use serde_json::{Value, json};

use crate::image_upload_upstream::{ImageUploadUpstream, UploadReply};
use crate::support::http_client::{self, SseReader};

const AXUM_DEFAULT_BODY_LIMIT_BYTES: usize = 2 * 1024 * 1024;

#[tokio::test(flavor = "multi_thread")]
async fn nonvisual_image_reaches_adapter_without_an_http_upload() {
    assert!(
        !DeepSeek.supports_images(),
        "DeepSeek 不消费 image_url，宿主不得为它上传图片"
    );
    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let mut template = support::http_server::session_template(upstream.chat_endpoint());
    template.upload_base_url = "http://127.0.0.1:1/inaccessible-upload-endpoint".to_string();
    template.api_key = "test-api-key".to_string();
    let (addr, sessions) = start(template).await;

    create(addr, "deepseek-with-image");
    let frames = turn(
        addr,
        "deepseek-with-image",
        r#"{"text":"请检查附件","images":[{"name":"receipt.png","mime":"image/png","bytes":[137,80,78,71]}]}"#,
    )
    .await;

    let request = chat_request(&upstream);
    assert_eq!(
        last_user_content(&request),
        &json!("请检查附件\n[用户上传了图片 receipt.png（image/png），当前模型看不到图片内容]"),
        "非视觉 adapter 必须拿到图片元数据并生成 083 的确定性占位文本"
    );
    assert_eq!(
        upstream.upload_count(),
        0,
        "不可访问的上传端点不应被触碰；DeepSeek 图片不能先上传再降级"
    );
    assert_eq!(
        upstream
            .calls()
            .iter()
            .map(|call| call.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/openai/v1/chat/completions"],
        "上游只能看到一次模型调用，不能混入 /files"
    );
    assert!(
        frames.iter().any(|frame| matches!(
            &frame.event,
            SessionEvent::TurnGuard { adjustments, .. }
                if adjustments == &vec![Adjustment::ImagesDropped { count: 1 }]
        )),
        "非视觉图片必须向 UI 精确报告 ImagesDropped {{ count: 1 }}：{frames:?}"
    );
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok()),
        "测试会话必须能干净关闭"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn json_over_axum_default_reaches_handler_when_raw_image_fits_the_quota() {
    const RAW_IMAGE_BYTES: usize = 1_100_000;

    let upstream = ImageUploadUpstream::start(UploadReply::Ok);
    let mut template = support::http_server::session_template(upstream.chat_endpoint());
    template.upload_base_url = "http://127.0.0.1:1/inaccessible-upload-endpoint".to_string();
    let (addr, sessions) = start(template).await;
    create(addr, "large-json-image");

    let body = image_request_body(RAW_IMAGE_BYTES);
    assert!(
        body.len() > AXUM_DEFAULT_BODY_LIMIT_BYTES,
        "回归请求必须真实超过 axum 默认 2 MiB：{} bytes",
        body.len()
    );
    let _frames = turn(addr, "large-json-image", &body).await;

    let request = chat_request(&upstream);
    assert_eq!(
        last_user_content(&request),
        &json!("检查大附件\n[用户上传了图片 large.png（image/png），当前模型看不到图片内容]"),
        "超过 2 MiB 的合法 JSON 必须穿过路由限制并到达 adapter"
    );
    assert_eq!(upstream.upload_count(), 0, "非视觉路径不得上传大附件");
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok()),
        "测试会话必须能干净关闭"
    );
}

fn image_request_body(raw_image_bytes: usize) -> String {
    let mut encoded_bytes = "0,".repeat(raw_image_bytes);
    encoded_bytes.pop();
    format!(
        r#"{{"text":"检查大附件","images":[{{"name":"large.png","mime":"image/png","bytes":[{encoded_bytes}]}}]}}"#
    )
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

fn create(addr: SocketAddr, id: &str) {
    let response = http_client::request(
        addr,
        "POST",
        "/sessions",
        Some(&json!({ "id": id }).to_string()),
    );
    assert_eq!(response.status, 201, "创建 {id} 失败：{}", response.body);
}

async fn turn(addr: SocketAddr, id: &str, body: &str) -> Vec<Frame> {
    let (_, _, mut sse) = http_client::connect_sse(addr, &format!("/sessions/{id}/events"), None);
    let response = http_client::request(addr, "POST", &format!("/sessions/{id}/input"), Some(body));
    assert_eq!(
        response.status, 202,
        "非视觉图片不得在上传阶段失败，必须进入 actor：{}",
        response.body
    );
    wait_for_terminal(&mut sse)
}

fn wait_for_terminal(sse: &mut SseReader) -> Vec<Frame> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut frames = Vec::new();
    while Instant::now() < deadline {
        let Some(event) = sse.next_event(deadline.saturating_duration_since(Instant::now())) else {
            break;
        };
        let frame: Frame = serde_json::from_str(&event.data)
            .unwrap_or_else(|error| panic!("SSE 不是 Frame：{error}: {}", event.data));
        let terminal = matches!(
            &frame.event,
            SessionEvent::Notice(Notice::TurnStatusChanged { status }) if status.is_terminal()
        );
        frames.push(frame);
        if terminal {
            return frames;
        }
    }
    panic!("等待输入轮终态超时，已收到：{frames:?}");
}

fn chat_request(upstream: &ImageUploadUpstream) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(body) = upstream.chat_bodies().first() {
            return serde_json::from_str(body)
                .unwrap_or_else(|error| panic!("模型请求不是 JSON：{error}\n{body}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "等待模型请求超时：{:?}",
        upstream
            .calls()
            .into_iter()
            .map(|call| call.path)
            .collect::<Vec<_>>()
    );
}

fn last_user_content(body: &Value) -> &Value {
    body["messages"]
        .as_array()
        .expect("DeepSeek 请求必须有 messages")
        .iter()
        .rev()
        .find(|message| message["role"] == "user")
        .map(|message| &message["content"])
        .expect("请求必须有 user 消息")
}
