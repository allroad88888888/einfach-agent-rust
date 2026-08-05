//! `McpClient` 的握手/应答匹配/翻译逻辑——用一段 `sh` 脚本假扮 MCP server，零
//! 网络依赖，快速确定。真起一次网络上的 server 是
//! `everything_server_042.rs` 的事（issue 042 验收原文要求的真集成测试）。
//!
//! 挪到这里而不是内联在 `src/client.rs`：红线 9（文件行数），见该文件模块文档。
//! 这里只用 `agent_mcp` 的公开 API（集成测试是独立编译的 crate，看不到
//! `pub(crate)` 的 `client::connect_fake_server` 捷径）。

use std::time::{Duration, Instant};

use agent_mcp::{CLIENT_PROTOCOL_VERSION, McpClient, McpError, ProtocolError, TransportError};
use agent_core::Reversibility;

fn connect(script: &str, handshake_timeout: Duration) -> Result<McpClient, McpError> {
    McpClient::connect(
        "sh",
        &["-c".to_string(), script.to_string()],
        &[],
        "agent-mcp-integration-test",
        "0.0.0",
        handshake_timeout,
    )
}

/// server 回一个跟本仓提议不一样的版本——握手照样成功、原样记下，不比较相等
/// （见 `client` 模块文档「协议版本：协商不是断言」）。
#[test]
fn connect_accepts_server_negotiated_protocol_version_without_asserting_equality() {
    let script = r#"read l1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2099-01-01","capabilities":{"tools":{}},"serverInfo":{"name":"fake"}}}'
read l2
"#;
    let client = connect(script, Duration::from_secs(5)).unwrap();
    assert_eq!(client.protocol_version, "2099-01-01");
    assert_ne!(client.protocol_version, CLIENT_PROTOCOL_VERSION);
    assert_eq!(client.server_name, Some("fake".to_string()));
}

/// 复现 042 实测：server 在 `tools/list` 的响应之前插播一条无 `id` 的通知，
/// 等响应的循环必须跳过它继续等，而不是把通知误当成响应、或者报错。
#[test]
fn list_tools_skips_interleaved_notification_and_translates_correctly() {
    let script = r#"read l1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}'
read l2
read l3
printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"echoes","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}}'
"#;
    let mut client = connect(script, Duration::from_secs(5)).unwrap();
    let (tools, warnings) = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(&*tools[0].0.name, "mcp:fakesrv/echo");
    assert_eq!(tools[0].1, Reversibility::Pure);
    assert!(warnings.is_empty(), "没有重复的工具名，不该有告警");
}

/// server 对 `tools/list` 回 JSON-RPC `error` 对象——`McpError::Rpc` 带上
/// code/message，不是笼统的字符串。
#[test]
fn server_error_object_surfaces_as_mcperror_rpc() {
    let script = r#"read l1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}'
read l2
read l3
printf '%s\n' '{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"method not found"}}'
"#;
    let mut client = connect(script, Duration::from_secs(5)).unwrap();
    let err = client.list_tools("fakesrv", Duration::from_secs(5)).unwrap_err();
    match err {
        McpError::Rpc { code, message } => {
            assert_eq!(code, -32601);
            assert_eq!(message, "method not found");
        }
        other => panic!("期望 McpError::Rpc，实际 {other:?}"),
    }
}

/// server 在握手响应之前就退出——`connect` 干净返回 `Err`，不 panic。
///
/// `McpClient` 故意不 derive `Debug`（红线 3 的活句柄不该被顺手打印/序列化），
/// 所以这里手动 match 取 `Err`，不用要求 `T: Debug` 的 `unwrap_err`。
#[test]
fn connect_fails_cleanly_when_server_exits_before_responding() {
    match connect("exit 0", Duration::from_secs(5)) {
        Err(err) => assert!(matches!(err, McpError::Transport(_))),
        Ok(_) => panic!("server 提前退出，握手不该成功"),
    }
}

/// server 起来了但永远不回应——握手要在预算内超时放弃，不永久挂起（红线：
/// 全局规则「后台跑 CLI 必须能超时放弃」的镜像，见 issue 042 验收原文）。
#[test]
fn connect_fails_cleanly_on_handshake_timeout() {
    let started = Instant::now();
    match connect("sleep 5", Duration::from_millis(200)) {
        Err(err) => assert!(
            matches!(err, McpError::Transport(TransportError::Timeout { .. })),
            "期望握手超时，实际 {err:?}"
        ),
        Ok(_) => panic!("server 从不回应，握手不该成功"),
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "握手超时应该在远小于原命令耗时内返回，实际 {:?}",
        started.elapsed()
    );
}

/// server 回一段不是合法 JSON 的垃圾——直接报协议错误，不是傻等到超时。
#[test]
fn garbage_non_json_response_surfaces_as_protocol_error_not_timeout() {
    let script = "read l1\nprintf 'not json at all\\n'\n";
    match connect(script, Duration::from_secs(5)) {
        Err(err) => assert!(
            matches!(err, McpError::Protocol(ProtocolError::NotJson(_))),
            "期望 NotJson，实际 {err:?}"
        ),
        Ok(_) => panic!("server 回了垃圾数据，握手不该成功"),
    }
}
