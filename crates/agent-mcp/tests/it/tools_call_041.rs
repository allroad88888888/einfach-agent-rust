//! 041 验收：`tools/call` 方法 params 的构造——`tools_call_params`。`name`
//! 必须是裸工具名，不带本仓自己起的 `mcp:<server>/` 前缀（server 不认识那个
//! 前缀）。
//!
//! 规格来源：`crates/agent-mcp/src/protocol.rs`（`tools_call_params` 文档
//! 注释）、`docs/issues/041-mcp-protocol.md` §验收「序列化一个 tools/call
//! request」一条。只测规格，不看实现体。

use agent_mcp::tools_call_params;
use serde_json::json;

#[test]
fn shape_is_bare_name_plus_arguments() {
    let params = tools_call_params("echo", json!({"message": "hi"}));
    assert_eq!(
        params,
        json!({"name": "echo", "arguments": {"message": "hi"}})
    );

    // name 必须是裸工具名，不带 mcp:<server>/ 前缀——那个前缀是本仓命名，
    // server 不认识；宿主在发之前剥掉。
    assert!(!params["name"].as_str().unwrap().starts_with("mcp:"));
}

#[test]
fn serializes_byte_identical_to_recorded_bytes() {
    // tools_call_params 返回 serde_json::Value，它的 Map 后端是 BTreeMap（顶层
    // serde_json 不开 preserve_order，红线 11）——序列化后的 key 顺序必然是
    // 字典序（"arguments" < "name"），与实现内部按什么顺序写字段无关。这是
    // 唯一能在不看实现体的前提下安全做逐字节比较的地方（encode_request 整个
    // 信封的字段顺序不受这个保证覆盖，见 jsonrpc_codec_041.rs 里改用结构性
    // 断言的说明）。
    let params = tools_call_params("add", json!({"a": 1, "b": 2}));
    let bytes = serde_json::to_vec(&params).unwrap();
    let recorded: &[u8] = br#"{"arguments":{"a":1,"b":2},"name":"add"}"#;
    assert_eq!(bytes, recorded);
}
