//! 会话生命周期端点（issue 031「做什么」小节）+ 错误形状分明（「404/409/410
//! （dead）分明」）：`GET /sessions/:id` 在活着/不存在/死了三种状态下分别报
//! 200(alive)/404/200(dead——状态本身不是错误,`GET` 就是用来问这个的);命令类
//! 端点在死会话上报 410,在不存在的 id 上报 404。

use crate::support;
use std::sync::Arc;

use agent_core::ErrorClass;
use agent_providers::{Decoded, Encoded, Ingredients, Provider, StreamAccumulator};
use serde_json::Value;

use crate::support::http_client;
use crate::support::server::FakeServer;

/// 跟 `actor_panic_is_reported_dead.rs` 同一招：`encode` 一被调用就 panic，
/// 不需要真的连网络就能可靠地把 actor 线程打死。
struct PanicProvider;

impl Provider for PanicProvider {
    fn encode(&self, _ing: &Ingredients<'_>) -> Encoded {
        panic!("boom-from-http-lifecycle-test")
    }
    fn decode(&self, _body: &Value) -> Decoded {
        unreachable!()
    }
    fn accumulator(&self) -> StreamAccumulator {
        unreachable!()
    }
    fn classify(&self, _status: u16, _body: &str) -> ErrorClass {
        unreachable!()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn creating_a_session_and_asking_its_status_reports_alive() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    assert_eq!(create.status, 201);
    assert!(
        create
            .header("content-type")
            .is_some_and(|v| v.starts_with("application/json"))
    );
    let id = support::extract_json_string_field(&create.body, "id");
    assert!(!id.is_empty());

    let status = http_client::request(server.addr, "GET", &format!("/sessions/{id}"), None);
    assert_eq!(status.status, 200);
    assert!(status.body.contains("\"alive\""), "{}", status.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_session_id_is_404_everywhere() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let status = http_client::request(server.addr, "GET", "/sessions/never-existed", None);
    assert_eq!(status.status, 404, "{}", status.body);
    assert!(
        status.body.contains("\"session_not_found\""),
        "{}",
        status.body
    );

    let input = http_client::request(
        server.addr,
        "POST",
        "/sessions/never-existed/input",
        Some("{\"text\":\"hi\"}"),
    );
    assert_eq!(input.status, 404, "{}", input.body);

    let cancel = http_client::request(
        server.addr,
        "POST",
        "/sessions/never-existed/cancel",
        Some("{}"),
    );
    assert_eq!(cancel.status, 404, "{}", cancel.body);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dead_session_is_410_not_404() {
    let upstream = FakeServer::start(vec![]); // 永远不会真的被连——`encode` 先 panic。
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.provider = Arc::new(PanicProvider);
    let server = support::http_server::start_at_with_template(
        "127.0.0.1:0".parse().unwrap(),
        template,
        |c| {
            c.with_ring_capacity(5)
                .with_cancel_grace(std::time::Duration::from_millis(200))
        },
    )
    .await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let input = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/input"),
        Some("{\"text\":\"hi\"}"),
    );
    assert_eq!(input.status, 202);

    // 等 actor 线程死透——轮询 `GET /sessions/:id` 直到不再是 alive。
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut last_status = String::new();
    while std::time::Instant::now() < deadline {
        let status = http_client::request(server.addr, "GET", &format!("/sessions/{id}"), None);
        last_status = status.body.clone();
        if status.status == 200 && status.body.contains("\"dead\"") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        last_status.contains("\"dead\""),
        "session 该在 provider 喂了畸形响应之后死掉：{last_status}"
    );

    let cancel = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/cancel"),
        Some("{}"),
    );
    assert_eq!(
        cancel.status, 410,
        "对一个已死的 session 发命令该是 410，不是别的：{}",
        cancel.body
    );
    assert!(cancel.body.contains("\"session_dead\""), "{}", cancel.body);
}
