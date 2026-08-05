//! 044 验收（装载层，起子进程但零网络）：多 server 合并、失败隔离、host 门。
//!
//! 「好」server 用一段 `sh` 脚本假扮一个 MCP server（回 initialize + tools/list 两帧，
//! 之后阻塞等被 kill），零网络依赖、确定可判定。「坏」server 用一个不存在的命令，spawn
//! 当场失败——正是失败隔离要隔离的东西。真起 `@modelcontextprotocol/server-everything`
//! 的端到端在 042 已覆盖，这里不重复起 npx（保持门禁快、不留 orphan）。

use std::collections::BTreeMap;
use std::time::Duration;

use agent_mcp::{
    Availability, Host, LoadTimeouts, McpConfig, McpRegistry, ServerConfig, StdioServer,
    load_servers,
};

/// 一段假扮 MCP server 的 `sh` 脚本：回 initialize（id 1）+ tools/list（id 2，含一个
/// 名叫 `tool_name` 的 readOnly 工具），然后阻塞等 stdin（进程不退出，直到 registry
/// drop 时被 kill+wait 收尸）。id 序列对齐 `McpClient`：initialize=1、tools/list=2。
fn fake_server(tool_name: &str) -> ServerConfig {
    let script = format!(
        "read a\n\
         printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}},\"serverInfo\":{{\"name\":\"fake\",\"version\":\"0\"}}}}}}'\n\
         read b\n\
         read c\n\
         printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[{{\"name\":\"{tool_name}\",\"description\":\"d\",\"inputSchema\":{{\"type\":\"object\"}},\"annotations\":{{\"readOnlyHint\":true}}}}]}}}}'\n\
         read d\n"
    );
    ServerConfig::Stdio(StdioServer {
        command: "sh".into(),
        args: vec!["-c".into(), script],
        env: BTreeMap::new(),
    })
}

fn nonexistent_server() -> ServerConfig {
    ServerConfig::Stdio(StdioServer {
        command: "this-command-does-not-exist-044".into(),
        args: vec![],
        env: BTreeMap::new(),
    })
}

fn timeouts() -> LoadTimeouts {
    LoadTimeouts { handshake: Duration::from_secs(5), call: Duration::from_secs(5) }
}

/// 两个 server 各有同名工具 `x` → 两批都进表，`mcp:a/x` 与 `mcp:b/x` 不撞。
#[test]
fn two_servers_merge_with_disambiguated_prefixes() {
    let config = McpConfig {
        servers: vec![("a".into(), fake_server("x")), ("b".into(), fake_server("x"))],
    };
    let registry = McpRegistry::new();
    let out = load_servers(&config, &registry, Host::Server, timeouts(), "t", "0");

    let names: Vec<String> = out.tools.iter().map(|(s, _)| s.name.to_string()).collect();
    assert!(names.contains(&"mcp:a/x".to_string()), "缺 mcp:a/x，实际 {names:?}");
    assert!(names.contains(&"mcp:b/x".to_string()), "缺 mcp:b/x，实际 {names:?}");
    assert_eq!(names.len(), 2, "两批工具都进表，不撞名");

    assert_eq!(out.connected_ids(), vec!["a", "b"]);
    assert!(registry.contains("a") && registry.contains("b"), "活句柄进了 registry");
}

/// 一个 server 的命令指向不存在的可执行文件 → 它标 Unavailable（带原因），另一个正常
/// server 的工具照常在表里，available_servers() 报出谁连上了谁没有 + 原因。
#[test]
fn one_bad_server_is_isolated_others_still_connect() {
    let config = McpConfig {
        servers: vec![
            ("good".into(), fake_server("echo")),
            ("bad".into(), nonexistent_server()),
        ],
    };
    let registry = McpRegistry::new();
    let out = load_servers(&config, &registry, Host::Server, timeouts(), "t", "0");

    // 好 server 的工具照常在表里。
    let names: Vec<String> = out.tools.iter().map(|(s, _)| s.name.to_string()).collect();
    assert_eq!(names, vec!["mcp:good/echo".to_string()]);
    assert!(registry.contains("good"));
    assert!(!registry.contains("bad"), "连不上的 server 不进 registry");

    // available_servers() 报出谁连上了、谁没有 + 原因。
    let statuses = out.available_servers();
    assert_eq!(statuses.len(), 2);
    let good = statuses.iter().find(|s| s.id == "good").unwrap();
    assert_eq!(good.availability, Availability::Connected { tool_count: 1 });
    let bad = statuses.iter().find(|s| s.id == "bad").unwrap();
    match &bad.availability {
        Availability::Unavailable { reason } => assert!(!reason.is_empty(), "要带原因"),
        other => panic!("bad server 应是 Unavailable，实际 {other:?}"),
    }
}

/// 远端 http 条目 → 标 Unsupported（带原因），其余 stdio server 照常。
#[test]
fn remote_entry_marked_unsupported_others_fine() {
    // 顺序：web（远端）先——证明它不阻塞后面的 local。
    let config = McpConfig {
        servers: vec![
            (
                "web".into(),
                ServerConfig::Remote(agent_mcp::RemoteServer {
                    transport_type: "http".into(),
                    url: "https://x".into(),
                    headers: BTreeMap::new(),
                }),
            ),
            ("local".into(), fake_server("ping")),
        ],
    };
    let registry = McpRegistry::new();
    let out = load_servers(&config, &registry, Host::Server, timeouts(), "t", "0");

    let web = out.available_servers().iter().find(|s| s.id == "web").unwrap();
    assert!(matches!(web.availability, Availability::Unsupported { .. }), "web 应暂不支持");
    assert!(!registry.contains("web"), "远端不进 registry");

    assert!(out.connected_ids().contains(&"local"), "stdio server 照常连上");
    assert!(out.tools.iter().any(|(s, _)| &*s.name == "mcp:local/ping"));
    // local 的子进程活句柄在 registry 里，registry 在函数末尾 drop → kill+wait 收尸。
}

/// host 门：stdio server 在浏览器 host 上标 Unavailable（不 spawn，形状留位）。M6 的
/// CLI 是 server host 走不到这里，但门要能表达。
#[test]
fn stdio_on_browser_host_is_unavailable_without_spawning() {
    let config =
        McpConfig { servers: vec![("a".into(), fake_server("noop"))] };
    let registry = McpRegistry::new();
    let out = load_servers(&config, &registry, Host::Browser, timeouts(), "t", "0");

    let a = &out.available_servers()[0];
    match &a.availability {
        Availability::Unavailable { reason } => assert!(reason.contains("browser")),
        other => panic!("浏览器 host + stdio 应 Unavailable，实际 {other:?}"),
    }
    assert!(out.tools.is_empty());
    assert!(!registry.contains("a"), "门不通过：不该 spawn、不该进 registry");
}
