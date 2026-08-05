//! 041 验收：`initialize` 方法 result 的解析——`parse_initialize_result`。
//!
//! 规格来源：`crates/agent-mcp/src/protocol.rs`（`InitializeResult` 字段 +
//! `parse_initialize_result` 文档注释）、`docs/issues/041-mcp-protocol.md`
//! §验收「录制的 initialize 响应」一条。只测规格，不看实现体。

mod common;

use agent_mcp::{ProtocolError, parse_initialize_result};
use common::recorded_result;
use serde_json::json;

#[test]
fn happy_path_extracts_version_capabilities_server_name() {
    let frame = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true},"resources":{}},"serverInfo":{"name":"everything","version":"1.0.0"}}}"#;
    let result = recorded_result(frame);
    let parsed = parse_initialize_result(&result).expect("合法 initialize result 应当解析成功");

    assert_eq!(parsed.protocol_version, "2025-06-18");
    assert_eq!(parsed.server_name.as_deref(), Some("everything"));
    assert_eq!(
        parsed.capabilities,
        json!({"tools": {"listChanged": true}, "resources": {}})
    );
}

#[test]
fn missing_protocol_version_is_unexpected_shape() {
    let frame = br#"{"jsonrpc":"2.0","id":1,"result":{"capabilities":{},"serverInfo":{"name":"everything"}}}"#;
    let result = recorded_result(frame);
    let err = parse_initialize_result(&result).expect_err("缺 protocolVersion 必须报错");
    assert!(
        matches!(err, ProtocolError::UnexpectedShape(_)),
        "应为 UnexpectedShape，实际是 {err:?}"
    );
}

#[test]
fn unknown_extra_fields_do_not_error() {
    // server 可能带我们还不认识的字段（`instructions`、`_meta` 等）——未知字段
    // 一律忽略、不报错、不丢弃到猜（协议要向前兼容）。
    let frame = br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"everything","version":"1.0.0"},"instructions":"be nice","_meta":{"vendor":"acme"}}}"#;
    let result = recorded_result(frame);
    let parsed = parse_initialize_result(&result).expect("未知额外字段不应报错");
    assert_eq!(parsed.protocol_version, "2025-06-18");
    assert_eq!(parsed.server_name.as_deref(), Some("everything"));
}

#[test]
fn missing_server_info_falls_back_to_none_not_error() {
    // server_name 类型是 Option<String>（文档注释：「可能缺」）——serverInfo
    // 整体缺失时必须落 None，不能报错。
    let frame =
        br#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}"#;
    let result = recorded_result(frame);
    let parsed = parse_initialize_result(&result)
        .expect("serverInfo 整体缺失不应报错，server_name 落 None");
    assert_eq!(parsed.server_name, None);
}
