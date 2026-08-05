//! 044 验收（解析层，零 IO）：`.mcp.json` 解析、撞名报错、远端只解析留位不崩、host
//! 可用性门的形状。装载/失败隔离在 `tests/mcp_loader_044.rs`（要起子进程）。

use agent_mcp::{
    ConfigError, Host, RemoteServer, ServerConfig, StdioServer, TransportKind, parse_config,
};

/// 两个 server 都解析进来，保序，各自的 stdio 字段对。
#[test]
fn two_server_config_parses_both_in_order() {
    let cfg = parse_config(
        r#"{"mcpServers":{
            "a":{"command":"npx","args":["-y","a-pkg"],"env":{"A":"1"}},
            "b":{"command":"node","args":["b.js"]}
        }}"#,
    )
    .unwrap();

    let ids: Vec<&str> = cfg.servers.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["a", "b"], "保序：a 先 b 后");

    let (_, ServerConfig::Stdio(a)) = &cfg.servers[0] else {
        panic!("a 应是 stdio")
    };
    assert_eq!(a.command, "npx");
    assert_eq!(a.args, vec!["-y", "a-pkg"]);
    assert_eq!(a.env_pairs(), vec![("A".to_string(), "1".to_string())]);
}

/// 撞名（同一个 server id 声明两次）→ 明确报错，不静默取后者。
#[test]
fn duplicate_server_id_is_a_clear_error() {
    let err = parse_config(
        r#"{"mcpServers":{
            "dup":{"command":"first"},
            "dup":{"command":"second"}
        }}"#,
    )
    .unwrap_err();
    match err {
        ConfigError::DuplicateServerId(id) => assert_eq!(id, "dup"),
        other => panic!("撞名应报 DuplicateServerId，实际 {other:?}"),
    }
}

/// 远端 `{type:"http",...}` 出现 → 解析不崩，标成 Remote 留位；同文件的 stdio 照常。
#[test]
fn remote_http_entry_parses_as_remote_without_crashing_other_stdio() {
    let cfg = parse_config(
        r#"{"mcpServers":{
            "web":{"type":"http","url":"https://example/mcp","headers":{"Authorization":"Bearer x"}},
            "local":{"command":"npx","args":["-y","local"]}
        }}"#,
    )
    .unwrap();
    assert_eq!(cfg.servers.len(), 2, "远端条目不该让整份解析失败");

    let (_, ServerConfig::Remote(r)) = &cfg.servers[0] else {
        panic!("web 应是 remote")
    };
    assert_eq!(r.transport_type, "http");
    assert_eq!(r.url, "https://example/mcp");

    assert!(
        matches!(&cfg.servers[1], (_, ServerConfig::Stdio(_))),
        "同文件的 stdio server 照常解析"
    );
}

/// `type:"sse"` 也当远端留位。
#[test]
fn sse_entry_parses_as_remote() {
    let cfg = parse_config(r#"{"mcpServers":{"s":{"type":"sse","url":"https://x/sse"}}}"#).unwrap();
    let (_, ServerConfig::Remote(r)) = &cfg.servers[0] else {
        panic!("应是 remote")
    };
    assert_eq!(r.transport_type, "sse");
}

/// host 可用性门：stdio 在 server host 可用；接口能表达不可用（stdio + 浏览器）；
/// 远端 + 浏览器延后但形状在（`available_on` 返回可用）。
#[test]
fn host_gate_available_on_expresses_availability() {
    let stdio = ServerConfig::Stdio(StdioServer {
        command: "npx".into(),
        args: vec![],
        env: Default::default(),
    });
    assert_eq!(stdio.transport_kind(), TransportKind::Stdio);
    assert!(
        stdio.available_on(Host::Server),
        "stdio + server host：可用"
    );
    assert!(
        !stdio.available_on(Host::Browser),
        "stdio + 浏览器：接口能表达不可用"
    );

    let remote = ServerConfig::Remote(RemoteServer {
        transport_type: "http".into(),
        url: "https://x".into(),
        headers: Default::default(),
    });
    assert_eq!(remote.transport_kind(), TransportKind::Remote);
    assert!(
        remote.available_on(Host::Browser),
        "远端 + 浏览器：延后但形状在（门返回可用）"
    );
    assert!(
        remote.available_on(Host::Server),
        "远端 + server host：门返回可用（M6 不实现是 loader 的判断）"
    );
}
