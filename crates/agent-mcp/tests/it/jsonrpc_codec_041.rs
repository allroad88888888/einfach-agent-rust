//! 041 验收：JSON-RPC 2.0 信封在合法输入下的编解码——`encode_request` /
//! `encode_notification` / `parse_response` 的快乐路径。畸形帧的错误路径见
//! `jsonrpc_malformed_041.rs`（拆开：happy-path 与 edge-case 是两个场景）。
//!
//! 规格来源：`crates/agent-mcp/src/jsonrpc.rs` 的 pub 类型/签名/文档注释、
//! `docs/issues/041-mcp-protocol.md` §验收。只测规格，不看实现体（写这份测试
//! 时函数体全是 `todo!()`）。

mod common;

use agent_mcp::{
    RpcResponse, encode_notification, encode_request, parse_response, tools_call_params,
};
use common::recorded_result;
use serde_json::{Value, json};

#[test]
fn parse_response_happy_path_result() {
    let bytes = br#"{"jsonrpc":"2.0","id":42,"result":{"foo":"bar"}}"#;
    let response = parse_response(bytes).expect("合法 result 响应应当解析成功");
    match response {
        RpcResponse::Result { id, result } => {
            assert_eq!(id, 42);
            assert_eq!(result, json!({"foo": "bar"}));
        }
        RpcResponse::Error { .. } => panic!("不该解析成 Error"),
    }
}

#[test]
fn parse_response_happy_path_error() {
    let bytes =
        br#"{"jsonrpc":"2.0","id":42,"error":{"code":-32601,"message":"Method not found"}}"#;
    let response = parse_response(bytes).expect("合法 error 响应应当解析成功");
    match response {
        RpcResponse::Error { id, error } => {
            assert_eq!(id, 42);
            assert_eq!(error.code, -32601);
            assert_eq!(error.message, "Method not found");
            assert_eq!(error.data, None);
        }
        RpcResponse::Result { .. } => panic!("不该解析成 Result"),
    }
}

/// `recorded_result` 帮手本身跑在 `parse_response` 之上，其余场景文件都靠它拿
/// `result` Value——这里顺带确认它对一个真实的 `tools/list` 空 result 帧也能
/// 正常工作（不是只对 `initialize` 帧凑巧能用）。
#[test]
fn recorded_result_helper_works_on_arbitrary_result_shape() {
    let bytes = br#"{"jsonrpc":"2.0","id":9,"result":{"anything":123}}"#;
    let result = recorded_result(bytes);
    assert_eq!(result, json!({"anything": 123}));
}

// 注：不对 encode_request 的整段输出字节做逐字面量比较。它返回 Vec<u8>（不是
// Value），实现既可能拼一个 Serialize 派生 struct（字段按声明顺序：
// jsonrpc/id/method/params）也可能经 serde_json::Value 再序列化（BTreeMap 会把
// 顶层键排成 id/jsonrpc/method/params 的字典序，和声明顺序不同）——两种写法都
// 满足 pub 签名与文档注释，字段顺序不是钉死的接口。所以这里解析回 Value 再逐
// 字段断言，只对 tools_call_params（签名钉死返回 Value，天然吃到 BTreeMap 的
// 逐字节保证）才做真正的字节比较，见 tools_call_041.rs。
#[test]
fn encode_request_round_trips_jsonrpc_id_method_params() {
    let params = tools_call_params("echo", json!({"message": "hi"}));
    let bytes = encode_request(7, "tools/call", Some(params.clone()));

    assert!(
        !bytes.ends_with(b"\n"),
        "encode_request 不应带换行——newline 分帧是 042 传输层的事"
    );

    let value: Value = serde_json::from_slice(&bytes).expect("encode_request 输出必须是合法 JSON");
    assert_eq!(value["jsonrpc"], json!("2.0"));
    assert_eq!(value["id"], json!(7));
    assert_eq!(value["method"], json!("tools/call"));
    assert_eq!(value["params"], params);
}

#[test]
fn encode_request_omits_params_field_when_none() {
    let bytes = encode_request(1, "notifications/initialized", None);
    let value: Value = serde_json::from_slice(&bytes).expect("必须是合法 JSON");
    assert!(
        value.get("params").is_none(),
        "params 为 None 时不应写 params 字段（文档注释明说：有些 server 对 params: null 敏感）"
    );
}

#[test]
fn encode_request_is_deterministic() {
    let a = encode_request(1, "tools/list", None);
    let b = encode_request(1, "tools/list", None);
    assert_eq!(a, b, "相同输入两次编码必须逐字节相同");
}

#[test]
fn encode_notification_has_no_id_field() {
    let bytes = encode_notification("notifications/initialized", None);
    let value: Value =
        serde_json::from_slice(&bytes).expect("encode_notification 输出必须是合法 JSON");
    assert!(value.get("id").is_none(), "notification 不应有 id 字段");
    assert_eq!(value["jsonrpc"], json!("2.0"));
    assert_eq!(value["method"], json!("notifications/initialized"));
    assert!(
        !bytes.ends_with(b"\n"),
        "encode_notification 不应带换行——newline 分帧是 042 传输层的事"
    );
}
