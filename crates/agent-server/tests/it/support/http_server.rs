//! 起一个真的 `AgentServer`（issue 031），绑 loopback 随机端口，后台跑到测试
//! 结束——`tests/support/http_client.rs` 的假浏览器连它。
#![allow(dead_code)]

use std::net::SocketAddr;
use std::time::Duration;

use agent_providers::deepseek::DeepSeek;
use agent_server::{AgentServer, ServerConfig, SessionTemplate};
use agent_transport::{Backoff, Client};

/// 后台跑着的测试服务器。持有 `addr` 给假浏览器连；不需要显式关闭——测试进程
/// 结束就没了（跟 `support::server::FakeServer` 的既有取舍一致）。
pub struct TestServer {
    pub addr: SocketAddr,
}

/// 默认配置：DeepSeek 假上游、5 帧的环形缓冲（测试要能故意撑爆它验证 gap，
/// 256 帧太大跑不动这类断言）、200ms 断开取消宽限期（不是 issue 原文的 5s——
/// 「可配」正是为了让测试不用真的等 5 秒）、100ms SSE 心跳（默认 15s 太长——
/// 断开检测在某些实现路径下要等下一次向 socket 写东西才会发现对面已经挂了，
/// 心跳短一点能让「断开多久之后才发现」这件事跟被测的宽限计时器解耦，不被
/// 「须要等到下一次心跳」这个无关变量拖长，尤其是在全 workspace 并发跑测试、
/// 调度延迟本来就比空闲环境大的时候）。
pub async fn start(endpoint: String) -> TestServer {
    start_with(endpoint, |c| {
        c.with_ring_capacity(5).with_cancel_grace(Duration::from_millis(200)).with_sse_keep_alive(Duration::from_millis(100))
    })
    .await
}

/// 需要非默认配置（比如断言默认宽限期真的是 5s 的那个测试）时用这个。
pub async fn start_with(endpoint: String, customize: impl FnOnce(ServerConfig) -> ServerConfig) -> TestServer {
    start_at("127.0.0.1:0".parse().unwrap(), endpoint, customize).await
}

/// [`start_with`] 的底层版本：连绑定地址都由调用方给——`http_bind_defaults_to_
/// loopback.rs` 要证明「不指定就是 loopback」这件事,不能走一条已经替它把地址
/// 硬编码成 `127.0.0.1` 的捷径,得真的经 `agent_server::default_bind_addr` 那条
/// 红线 8 的路径。
pub async fn start_at(addr: SocketAddr, endpoint: String, customize: impl FnOnce(ServerConfig) -> ServerConfig) -> TestServer {
    start_at_with_template(addr, session_template(endpoint), customize).await
}

/// 需要换掉 provider（比如故意造一个会 panic 的假 provider,逼 actor 线程真的
/// 死掉,验证「死会话报 410」）时用这个——先拿 [`session_template`] 现成的一份，
/// 改 `.provider` 字段,再喂进来。
pub async fn start_at_with_template(addr: SocketAddr, template: SessionTemplate, customize: impl FnOnce(ServerConfig) -> ServerConfig) -> TestServer {
    let config = customize(ServerConfig::new(template));
    let server = AgentServer::new(config);
    let bound = server.bind(addr).await.expect("bind 测试服务器");
    let addr = bound.local_addr();
    tokio::spawn(bound.serve());
    TestServer { addr }
}

/// 跟 `support::open_spec` 同一套参数选择（DeepSeek、短连接超时/取消轮询节奏），
/// 只是形状是 `SessionTemplate`（少 `id`/`store_path`，031 的 `POST /sessions`
/// 只认 `session_path`，`id` 由服务端生成）。`pub`——`start_at_with_template`
/// 的调用方常常只想改一个字段（比如 provider），不想把其余七个字段重抄一遍。
pub fn session_template(endpoint: String) -> SessionTemplate {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    SessionTemplate {
        provider: std::sync::Arc::new(DeepSeek),
        endpoint,
        api_key: "fake-key".to_string(),
        model: std::sync::Arc::from("deepseek-v4-pro"),
        tools: agent_server::ToolTableSpec::Builtin,
        tools_root: super::temp_dir("http-tools-root"),
        system: vec![agent_core::SystemChunk { label: std::sync::Arc::from("base"), text: std::sync::Arc::from("test") }],
        client: std::sync::Arc::new(client),
        history_cap: None,
        snapshot_every: Some(0),
        provider_timeout: Some(Duration::from_secs(5)),
        remote_tool_timeout: None,
        default_sessions_dir: None,
    }
}
