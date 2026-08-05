//! 验收清单第八条：`X-Accel-Buffering: no` 与 `Cache-Control: no-cache` 两个
//! header 在每一个 SSE 响应上都要有——ARCHITECTURE.md §传输：企业中间层（nginx
//! / Ingress / 内部 LB）默认缓冲会把流式响应变成一次性吐完，server 一次发对
//! 全链路才老实。

mod support;

use support::http_client;
use support::server::FakeServer;

#[tokio::test(flavor = "multi_thread")]
async fn every_sse_response_carries_both_headers() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let (status, headers, _sse) =
        http_client::connect_sse(server.addr, &format!("/sessions/{id}/events"), None);
    assert_eq!(status, 200);

    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        get("cache-control").as_deref(),
        Some("no-cache"),
        "{headers:?}"
    );
    assert_eq!(
        get("x-accel-buffering").as_deref(),
        Some("no"),
        "{headers:?}"
    );
    assert!(
        get("content-type").is_some_and(|v| v.starts_with("text/event-stream")),
        "{headers:?}"
    );
}
