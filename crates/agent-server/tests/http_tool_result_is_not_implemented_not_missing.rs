//! 验收清单第六条：`POST /sessions/:id/tool_result` 是明确的 501（前端工具是
//! 033 之后的事），不是 404——哪怕 session id 压根不存在也一样（`crate::http::
//! routes::tool_result` 模块文档：这条路径本身没准备好接住任何调用，不该先查
//! session 状态制造一个掩盖问题的 404/410）。

mod support;

use support::http_client;
use support::server::FakeServer;

#[tokio::test(flavor = "multi_thread")]
async fn tool_result_endpoint_returns_501_with_an_explicit_message() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let body = "{\"tool_call_id\":\"x\",\"epoch\":1,\"result\":{}}";
    let real_session = http_client::request(server.addr, "POST", &format!("/sessions/{id}/tool_result"), Some(body));
    assert_eq!(real_session.status, 501, "{}", real_session.body);
    assert!(real_session.body.contains("\"not_implemented\""), "{}", real_session.body);
    assert!(real_session.body.contains("033"), "错误消息该点名『前端工具/033』这类明确说法：{}", real_session.body);

    let unknown_session = http_client::request(server.addr, "POST", "/sessions/does-not-exist/tool_result", Some(body));
    assert_eq!(unknown_session.status, 501, "哪怕 session 不存在，也该是 501 不是 404：{}", unknown_session.body);
}
