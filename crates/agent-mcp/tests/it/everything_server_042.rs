//! 042 集成测试：真起一次 `npx -y @modelcontextprotocol/server-everything`，
//! 走完握手，`tools/list` 拿到真工具，翻译出的 name 全是 `mcp:everything/<t>`
//! （issue 042 验收原文）。
//!
//! **无 npx / 无网络 → skip 且打印原因，不静默假过**：两道预检——`npx
//! --version` 起得来、`npm view` 这个包在当前配置的 registry 上拉得到——任何
//! 一道过不去就打印原因、`return`（Rust 没有内建的「跳过」测试状态，这是本仓
//! 对这条约束的落地方式：日志说明 + 提前返回，跟「测试真的跑过且断言全过」
//! 区分开，不是伪装成通过）。**两道预检都过了之后的任何失败都是真失败**，
//! 直接 `panic` 出来，不吞——不能因为「可能是环境问题」就把真 bug 也悄悄咽掉。

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use agent_mcp::McpClient;

const PACKAGE: &str = "@modelcontextprotocol/server-everything";

#[test]
fn everything_server_handshake_and_tools_list() {
    if !npx_available() {
        eprintln!("skip: 本机没有 npx，042 集成测试跳过");
        return;
    }
    if !package_reachable() {
        eprintln!(
            "skip: {PACKAGE} 在当前 npm registry 上拉不到（无网络/registry 不可达），\
             042 集成测试跳过"
        );
        return;
    }

    let mut client = McpClient::connect(
        "npx",
        &["-y".to_string(), PACKAGE.to_string()],
        &[],
        "agent-mcp-integration-test",
        "0.1.0",
        Duration::from_secs(120),
    )
    .expect("真起 everything server 握手应当成功");

    eprintln!("server 协商的协议版本: {}", client.protocol_version);
    eprintln!("server 自报名字: {:?}", client.server_name);
    assert!(!client.protocol_version.is_empty(), "协议版本不能是空串");

    let (translated, warnings) = client
        .list_tools("everything", Duration::from_secs(30))
        .expect("tools/list 应当成功");

    assert!(!translated.is_empty(), "everything server 至少要有一个工具");
    assert!(warnings.is_empty(), "真实 everything server 不该有重复工具名: {warnings:?}");
    for (spec, _reversibility) in &translated {
        assert!(
            spec.name.starts_with("mcp:everything/"),
            "翻译出的名字必须是 mcp:everything/<t>，实际: {}",
            spec.name
        );
    }
    eprintln!(
        "翻译出的工具名: {:?}",
        translated.iter().map(|(s, _)| s.name.to_string()).collect::<Vec<_>>()
    );
}

fn npx_available() -> bool {
    Command::new("npx")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 有界探测 registry 是不是拉得到这个包——不能让一次没有网络的探测本身永久
/// 挂住测试。用 `try_wait` 轮询而不是「后台线程 + recv_timeout」：这样超时时
/// 我们自己直接 kill+wait 掉这个探测子进程，不留一个还在跑的 `npm view`。
fn package_reachable() -> bool {
    let mut child = match Command::new("npm")
        .args(["view", PACKAGE, "version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => return false,
        }
    }
}
