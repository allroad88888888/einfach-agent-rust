//! 041 验收：畸形 JSON-RPC 帧必须落到具体的 `ProtocolError` 变体，**不猜成成功**。
//! 这是 `parse_response` 的错误路径，和快乐路径拆开在
//! `jsonrpc_codec_041.rs`——本文件是独立测试 agent 的重点之一：实现体只覆盖
//! 作者想到的畸形形状，这里穷举 issue 验收点名的三类 + 文档注释额外点名的一类。
//!
//! 规格来源：`crates/agent-mcp/src/error.rs`（`ProtocolError` 变体定义）、
//! `crates/agent-mcp/src/jsonrpc.rs`（`parse_response` 文档注释）、
//! `docs/issues/041-mcp-protocol.md` §验收「畸形 JSON-RPC」一条。

use agent_mcp::{ProtocolError, parse_response};

#[test]
fn not_json_is_not_json_error() {
    let bytes = b"this is not json at all {{{";
    let err = parse_response(bytes).expect_err("非 JSON 必须报错");
    assert!(
        matches!(err, ProtocolError::NotJson(_)),
        "应为 NotJson，实际是 {err:?}"
    );
}

#[test]
fn missing_id_is_not_jsonrpc_error() {
    let bytes = br#"{"jsonrpc":"2.0","result":{"ok":true}}"#;
    let err = parse_response(bytes).expect_err("缺 id 必须报错");
    assert!(
        matches!(err, ProtocolError::NotJsonRpc(_)),
        "应为 NotJsonRpc，实际是 {err:?}"
    );
}

#[test]
fn non_integer_id_is_not_jsonrpc_error() {
    // 文档注释明说：「id 类型不对 → NotJsonRpc」。RpcResponse 把 id 钉死成
    // u64，字符串 id（JSON-RPC 规范允许，但本仓不支持）落这一类。
    let bytes = br#"{"jsonrpc":"2.0","id":"abc","result":{"ok":true}}"#;
    let err = parse_response(bytes).expect_err("id 不是整数必须报错");
    assert!(
        matches!(err, ProtocolError::NotJsonRpc(_)),
        "应为 NotJsonRpc，实际是 {err:?}"
    );
}

#[test]
fn both_result_and_error_present_is_malformed() {
    let bytes =
        br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true},"error":{"code":-1,"message":"boom"}}"#;
    let err = parse_response(bytes).expect_err("result 与 error 同时存在必须报错");
    assert!(
        matches!(err, ProtocolError::Malformed(_)),
        "应为 Malformed，实际是 {err:?}"
    );
}

#[test]
fn neither_result_nor_error_present_is_malformed() {
    let bytes = br#"{"jsonrpc":"2.0","id":1}"#;
    let err = parse_response(bytes).expect_err("result 与 error 都不在必须报错");
    assert!(
        matches!(err, ProtocolError::Malformed(_)),
        "应为 Malformed，实际是 {err:?}"
    );
}
