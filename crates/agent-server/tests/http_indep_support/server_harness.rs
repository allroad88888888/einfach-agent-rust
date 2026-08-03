//! 起一个真的 `agent_server::AgentServer`（031 的 HTTP 面）指向假上游，绑
//! `127.0.0.1:0`，后台 `tokio::spawn` 掉 `serve`。只用 `lib.rs`/`command.rs`/
//! `event.rs`/`handle.rs` 暴露的公开面拼这套装配——`SessionTemplate` 各字段的
//! 具体形状是通过编译器报错试出来的（跟实现方一样的公开类型，但没看
//! `http/` 源码怎么用它），符合独测的「自己拼线」精神。

#![allow(dead_code)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use agent_core::SystemChunk;
use agent_providers::deepseek::DeepSeek;
use agent_server::{AgentServer, ServerConfig, SessionTemplate, ToolTableSpec};
use agent_transport::{Backoff, Client};

use super::raw_http::{Response, post_json, request};

pub struct TestServer {
    pub addr: SocketAddr,
}

pub struct HarnessConfig {
    pub ring_capacity: usize,
    pub cancel_grace: Duration,
    pub sse_keep_alive: Duration,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        HarnessConfig { ring_capacity: 256, cancel_grace: Duration::from_secs(5), sse_keep_alive: Duration::from_secs(30) }
    }
}

fn template(endpoint: String, tools_root: std::path::PathBuf) -> SessionTemplate {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    SessionTemplate {
        provider: Arc::new(DeepSeek),
        endpoint,
        api_key: "fake-key".to_string(),
        model: Arc::from("deepseek-v4-pro"),
        tools: ToolTableSpec::Builtin,
        tools_root,
        system: vec![SystemChunk { label: Arc::from("base"), text: Arc::from("independent http test") }],
        client: Arc::new(client),
        history_cap: None,
        snapshot_every: Some(0),
        provider_timeout: Some(Duration::from_secs(5)),
        default_sessions_dir: None,
    }
}

/// 起一个绑定在给定 `bind_addr` 上的服务器（红线 8 测试要控制绑哪个 IP，
/// 其余测试固定用 `127.0.0.1:0`——见 `start_on`）。`serve()` 在后台
/// `tokio::spawn` 掉，函数返回时监听已就绪（`bind` 完成）。
pub async fn start_on(bind_addr: SocketAddr, endpoint: String, config: HarnessConfig) -> TestServer {
    let tools_root = super::temp_dir("tools");
    let server_config = ServerConfig::new(template(endpoint, tools_root))
        .with_ring_capacity(config.ring_capacity)
        .with_cancel_grace(config.cancel_grace)
        .with_sse_keep_alive(config.sse_keep_alive);
    let server = AgentServer::new(server_config);
    let bound = server.bind(bind_addr).await.unwrap_or_else(|e| panic!("bind {bind_addr} 失败：{e}"));
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    // 给 accept 循环一点时间真正开始监听读写（bind 本身已经完成，这里只是
    // 让 tokio::spawn 出去的任务被调度到）。
    tokio::time::sleep(Duration::from_millis(20)).await;
    TestServer { addr }
}

pub async fn start(endpoint: String, config: HarnessConfig) -> TestServer {
    start_on("127.0.0.1:0".parse().unwrap(), endpoint, config).await
}

impl TestServer {
    pub fn create_session(&self) -> String {
        let resp = post_json(self.addr, "/sessions", "{}");
        assert_eq!(resp.status, 201, "POST /sessions 该 201，body={}", resp.body_str());
        resp.json()["id"].as_str().expect("响应体没有 id 字段").to_string()
    }

    pub fn create_session_with_store_path(&self, path: &str) -> String {
        let body = format!("{{\"session_path\":\"{path}\"}}");
        let resp = post_json(self.addr, "/sessions", &body);
        assert_eq!(resp.status, 201, "POST /sessions 该 201，body={}", resp.body_str());
        resp.json()["id"].as_str().expect("响应体没有 id 字段").to_string()
    }

    pub fn post_input(&self, id: &str, text: &str) -> Response {
        let body = serde_json::json!({ "text": text }).to_string();
        post_json(self.addr, &format!("/sessions/{id}/input"), &body)
    }

    pub fn post_undo(&self, id: &str, granularity: &str, force: bool) -> Response {
        let body = serde_json::json!({ "granularity": granularity, "force": force }).to_string();
        post_json(self.addr, &format!("/sessions/{id}/undo"), &body)
    }

    pub fn post_redo(&self, id: &str) -> Response {
        post_json(self.addr, &format!("/sessions/{id}/redo"), "{}")
    }

    pub fn post_cancel(&self, id: &str) -> Response {
        post_json(self.addr, &format!("/sessions/{id}/cancel"), "{}")
    }

    pub fn post_tool_result(&self, id: &str) -> Response {
        let body = serde_json::json!({ "agent": "root", "tool_call_id": "x", "result": { "content": "x" } }).to_string();
        post_json(self.addr, &format!("/sessions/{id}/tool_result"), &body)
    }

    pub fn get_status(&self, id: &str) -> Response {
        request(self.addr, "GET", &format!("/sessions/{id}"), &[], None)
    }
}
