//! 085：图片上传等待期间，HTTP session actor 仍须处理后续纯文本输入。

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
async fn a_slow_upload_does_not_hold_another_session_actor() {
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
            Some(r#"{"text":"慢上传","images":[{"mime":"image/png","bytes":[1]}]}"#),
        )
    });
    wait_for_upload(&upstream);

    let started = Instant::now();
    turn(addr, "plain-during-upload", r#"{"text":"不等图片"}"#).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "另一会话的 actor 被上传卡住了，普通输入不该等 700ms 上传结束"
    );
    let image_response = image_post.await.expect("图片 POST 线程");
    assert_eq!(
        image_response.status, 202,
        "慢上传最后仍要成功：{}",
        image_response.body
    );
    assert!(
        sessions
            .close_all()
            .iter()
            .all(|(_, result)| result.is_ok())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_upload_leaves_its_session_actor_available_until_the_reference_is_ready() {
    let upstream = ImageUploadUpstream::start(UploadReply::SlowOk(Duration::from_millis(700)));
    let (addr, sessions) = start(template(
        upstream.chat_endpoint(),
        upstream.upload_base_url(),
        support::temp_dir("image-ingress-own-actor"),
    ))
    .await;
    create(addr, "shared-actor");

    let image_addr = addr;
    let image_post = tokio::task::spawn_blocking(move || {
        http_client::request(
            image_addr,
            "POST",
            "/sessions/shared-actor/input",
            Some(r#"{"text":"慢上传","images":[{"mime":"image/png","bytes":[1]}]}"#),
        )
    });
    wait_for_upload(&upstream);

    let started = Instant::now();
    turn(addr, "shared-actor", r#"{"text":"不等图片"}"#).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "上传等待落进 session actor，队列后的纯文本不该等 700ms"
    );
    let image_response = image_post.await.expect("图片 POST 线程");
    assert_eq!(image_response.status, 202, "慢上传最终必须成功");
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

fn wait_for_upload(upstream: &ImageUploadUpstream) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if upstream.upload_count() == 1 {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("慢上传请求没有到假服务");
}
