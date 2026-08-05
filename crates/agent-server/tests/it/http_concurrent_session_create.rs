//! 同一业务 chatid 的并发 `POST /sessions` 必须是原子 get-or-create。

use std::sync::{Arc, Barrier};

use agent_server::{AgentServer, ServerConfig};

use crate::support;
use crate::support::http_client;
use crate::support::server::FakeServer;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_of_same_chatid_returns_one_created_and_rest_existing() {
    const CLIENTS: usize = 16;
    let upstream = FakeServer::start(vec![]);
    let mut template = support::http_server::session_template(upstream.endpoint());
    template.default_sessions_dir = Some(support::temp_dir("concurrent-chatid"));
    let server = AgentServer::new(
        ServerConfig::new(template)
            .with_private_capability(support::http_server::PRIVATE_CAPABILITY),
    );
    let sessions = server.sessions();
    let bound = server.bind("127.0.0.1:0".parse().unwrap()).await.unwrap();
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());

    let barrier = Arc::new(Barrier::new(CLIENTS));
    let mut requests = Vec::new();
    for _ in 0..CLIENTS {
        let barrier = Arc::clone(&barrier);
        requests.push(tokio::task::spawn_blocking(move || {
            barrier.wait();
            let body = serde_json::json!({
                "id": "shared-browser-chat",
                "capabilities": {
                    "tools": [{
                        "name": "web:dogfood/echo",
                        "description": "浏览器回显"
                    }]
                }
            })
            .to_string();
            http_client::request(addr, "POST", "/sessions", Some(&body))
        }));
    }

    let mut statuses = Vec::new();
    let mut outcomes = Vec::new();
    for request in requests {
        let response = request.await.unwrap();
        statuses.push(response.status);
        outcomes.push(support::extract_json_string_field(
            &response.body,
            "outcome",
        ));
    }
    assert_eq!(statuses.iter().filter(|&&status| status == 201).count(), 1);
    assert_eq!(statuses.iter().filter(|&&status| status == 200).count(), 15);
    assert_eq!(
        outcomes.iter().filter(|value| *value == "created").count(),
        1
    );
    assert_eq!(
        outcomes.iter().filter(|value| *value == "existing").count(),
        15
    );
    assert_eq!(sessions.ids().len(), 1, "只能创建一个 actor");
    assert!(sessions.close_all()[0].1.is_ok());
}
