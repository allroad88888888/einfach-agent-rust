//! `POST /sessions/:id/tool_result` 接受来自 Web 宿主的受限回传：活 session 收到
//! 202，未知 session 如同其它命令端点一样返回 404。调用槽位的精确校验由 actor
//! 异步完成，路由不在 HTTP 线程伪造同步成功。

mod support;

use support::http_client;
use support::server::FakeServer;

#[tokio::test(flavor = "multi_thread")]
async fn tool_result_endpoint_queues_a_web_result_for_an_existing_session() {
    let upstream = FakeServer::start(vec![]);
    let server = support::http_server::start(upstream.endpoint()).await;

    let create = http_client::request(server.addr, "POST", "/sessions", Some("{}"));
    let id = support::extract_json_string_field(&create.body, "id");

    let body = "{\"agent\":\"root\",\"tool_call_id\":\"x\",\"result\":{\"content\":\"done\"}}";
    let real_session = http_client::request(
        server.addr,
        "POST",
        &format!("/sessions/{id}/tool_result"),
        Some(body),
    );
    assert_eq!(real_session.status, 202, "{}", real_session.body);

    let unknown_session = http_client::request(
        server.addr,
        "POST",
        "/sessions/does-not-exist/tool_result",
        Some(body),
    );
    assert_eq!(
        unknown_session.status, 404,
        "未知 session 应与其它命令端点一致：{}",
        unknown_session.body
    );
}
