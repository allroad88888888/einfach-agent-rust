//! 验收清单第七条：绑定默认 loopback（红线 8）——不设 `AGENT_BIND` 时监听的
//! 是 `127.0.0.1`，不是「全部网卡」，绑一个随机端口验证实际监听地址。
//!
//! **不摸 `AGENT_BIND` 环境变量**（`crate::bind` 模块文档的理由：`cargo test`
//! 并发跑各个测试函数，`std::env::set_var` 在 2024 edition 是 `unsafe fn`，
//! 多线程下改进程级状态本身就不安全）——纯函数层面的覆盖行为已经在
//! `agent_server::resolve_bind_ip` 的单元测试里钉死了，这里只验证「真的绑起来
//! 一个 `AgentServer` 时，默认路径确实用的是 loopback」这条集成层面的事实，
//! 前提是这个进程的环境本来就没设 `AGENT_BIND`（本仓 CI/沙箱环境的既有假设）。

use crate::support;
use agent_server::default_bind_addr;

use crate::support::server::FakeServer;

#[tokio::test(flavor = "multi_thread")]
async fn a_freshly_bound_server_listens_on_loopback_by_default() {
    assert!(
        std::env::var("AGENT_BIND").is_err(),
        "这条测试假设环境里没设 AGENT_BIND，见本文件顶部注释"
    );

    let upstream = FakeServer::start(vec![]);
    // 走 `default_bind_addr`（红线 8 那条路径），不是测试帮手为了方便硬编码的
    // `127.0.0.1:0`——这才是真的在证明「不设 AGENT_BIND 就是 loopback」。
    let addr = default_bind_addr(0).expect("默认路径不该失败");
    let server = support::http_server::start_at(addr, upstream.endpoint(), |c| c).await;

    assert!(
        server.addr.ip().is_loopback(),
        "默认绑定地址该是 loopback，实际是 {}",
        server.addr.ip()
    );
    assert_ne!(server.addr.port(), 0, "起服务之后该有一个真实分配到的端口");
}

#[tokio::test]
async fn default_bind_addr_helper_resolves_to_loopback_with_the_given_port() {
    assert!(
        std::env::var("AGENT_BIND").is_err(),
        "这条测试假设环境里没设 AGENT_BIND"
    );
    let addr = default_bind_addr(0).expect("默认路径不该失败");
    assert!(addr.ip().is_loopback());
    assert_eq!(addr.port(), 0);
}
