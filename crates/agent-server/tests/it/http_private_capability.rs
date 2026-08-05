//! 私有 session API 必须由 Java 启动时交付的 capability 保护。这份矩阵把
//! 缺失、错误、正确三种请求逐端钉住。

use crate::support;
use crate::support::http_client;

const WRONG_CAPABILITY: &str = "wrong-private-capability";
const TOOL_BODY: &str = r#"{"agent":"root","tool_call_id":"call","claim_id":"claim"}"#;
const RESULT_BODY: &str = r#"{"agent":"root","tool_call_id":"call","claim_id":"claim","submission_id":"submission","outcome":{"status":"succeeded","content":"ok"}}"#;

#[tokio::test(flavor = "multi_thread")]
async fn private_routes_reject_missing_and_wrong_capability() {
    let upstream = support::server::FakeServer::start(Vec::new());
    let server = support::http_server::start(upstream.endpoint()).await;

    for (method, path, body) in [
        ("POST", "/sessions", Some("{}")),
        ("GET", "/sessions/absent", None),
        ("POST", "/sessions/absent/tool_claim", Some(TOOL_BODY)),
        ("POST", "/sessions/absent/tool_result", Some(RESULT_BODY)),
        (
            "GET",
            "/sessions/absent/tool_status?agent=root&tool_call_id=call",
            None,
        ),
        ("POST", "/sessions/absent/cancel", Some("{}")),
    ] {
        assert_denied(server.addr, method, path, body, &[]);
        assert_denied(
            server.addr,
            method,
            path,
            body,
            &[("x-agent-server-capability", WRONG_CAPABILITY)],
        );
        let accepted = http_client::request_exact_headers(
            server.addr,
            method,
            path,
            &[(
                "x-agent-server-capability",
                support::http_server::PRIVATE_CAPABILITY,
            )],
            body,
        );
        assert_ne!(accepted.status, 401, "{method} {path}: {}", accepted.body);
    }
}

fn assert_denied(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(&str, &str)],
) {
    let response = http_client::request_exact_headers(addr, method, path, headers, body);
    assert_eq!(response.status, 401, "{method} {path}: {}", response.body);
    assert!(
        !response
            .body
            .contains(support::http_server::PRIVATE_CAPABILITY),
        "鉴权错误不得回显 capability"
    );
}
