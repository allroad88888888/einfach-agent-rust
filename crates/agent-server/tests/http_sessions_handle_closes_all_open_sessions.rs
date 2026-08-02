//! `AgentServer::sessions()`/`SessionsHandle::close_all`（035）：这是
//! `agent-server-bin` 的 Ctrl-C 优雅退出真正会调用的那条路径，这里从
//! `POST /sessions` 开始整条链路走一遍，证明它确实关得掉——不只是委托关系
//! 对不对（`registry_ids_lists_open_sessions.rs` 钉住了 registry 那一层），
//! 而是「经真实 HTTP 创建的会话，句柄能拿到、`close_all` 之后它们真的从
//! registry 摘掉了（`GET /sessions/:id` 由 200 alive 变成 404 not-found）」。

mod support;

use std::time::Duration;

use support::http_server::session_template;

#[tokio::test(flavor = "multi_thread")]
async fn close_all_closes_every_session_created_over_http() {
    let template = session_template("http://127.0.0.1:1/unused".to_string());
    let server = agent_server::AgentServer::new(agent_server::ServerConfig::new(template));
    // 跟 `AgentServer::sessions` 文档说的用法一致：在 `bind` 消费掉 `server`
    // 之前先借出这份把手。
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    tokio::time::sleep(Duration::from_millis(20)).await;

    let a = support::http_client::request(addr, "POST", "/sessions", Some("{}"));
    assert_eq!(a.status, 201, "body={}", a.body);
    let b = support::http_client::request(addr, "POST", "/sessions", Some("{}"));
    assert_eq!(b.status, 201, "body={}", b.body);
    let id_a = support::extract_json_string_field(&a.body, "id");
    let id_b = support::extract_json_string_field(&b.body, "id");
    assert!(!id_a.is_empty() && !id_b.is_empty(), "两个 id 都该非空：{id_a:?} {id_b:?}");

    let alive_before = sessions.ids().iter().map(|id| id.to_string()).collect::<Vec<_>>();
    assert!(alive_before.contains(&id_a) && alive_before.contains(&id_b), "两个新建的 session 该出现在 ids() 里：{alive_before:?}");

    let outcomes = sessions.close_all();
    assert!(outcomes.iter().all(|(_, r)| r.is_ok()), "两个都是干净的活会话，close 不该报错：{outcomes:?}");
    assert!(sessions.ids().is_empty(), "close_all 之后 registry 该清空");

    for id in [&id_a, &id_b] {
        let status = support::http_client::request(addr, "GET", &format!("/sessions/{id}"), None);
        assert_eq!(status.status, 404, "close 之后再查该是 404 not-found，body={}", status.body);
    }
}
