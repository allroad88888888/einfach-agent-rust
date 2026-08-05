//! JSON-RPC 2.0 信封的编解码。**只管信封**——MCP 各方法的 result 形状在 `protocol`，
//! newline 分帧在传输层（042）。
//!
//! 041 的边界：这里编码出的是**不含换行的 JSON body**。stdio 传输按 newline-delimited
//! 分帧是 042 的事——把换行揉进这里会让「一条消息」和「一帧」耦合，http 传输（延后）
//! 不用换行分帧，届时就得拆回来。
//!
//! **编码用 `serde_json::Value`（`Map` = `BTreeMap`）而不是手拼字符串**：跟
//! `agent-providers/src/deepseek/encode.rs` 的 `body()` 同一个套路——不追求匹配某个
//! 外部实现的字段顺序，只要「同样的输入两次编码逐字节相同」（红线 11 的精神），键序
//! 交给 `BTreeMap` 按字典序自动定，全仓统一，不必每处手工控顺序。

use serde_json::{Map, Value, json};

use crate::error::ProtocolError;

/// 一条 JSON-RPC 2.0 响应。notification（无 `id`）不是响应，不在这里。
///
/// `Result` 与 `Error` 互斥——信封里两者同时出现或都不出现都是 [`ProtocolError::Malformed`]
/// （见 [`parse_response`]）。
#[derive(Debug, Clone, PartialEq)]
pub enum RpcResponse {
    /// 成功：`id` 对上请求，`result` 是方法自己的形状（交给 `protocol` 解析）。
    Result { id: u64, result: Value },
    /// server 报错：JSON-RPC 层的 `error` 对象。
    Error { id: u64, error: RpcError },
}

/// JSON-RPC 2.0 的 error 对象。
#[derive(Debug, Clone, PartialEq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<Value>,
}

/// 编码一条 request 的 JSON body（`jsonrpc`/`id`/`method`/`params`）。**不含换行。**
///
/// `params` 为 `None` 时不写 `params` 字段（有些 server 对 `params: null` 敏感）。
pub fn encode_request(id: u64, method: &str, params: Option<Value>) -> Vec<u8> {
    let mut obj = Map::new();
    obj.insert("jsonrpc".to_string(), json!("2.0"));
    obj.insert("id".to_string(), json!(id));
    obj.insert("method".to_string(), json!(method));
    if let Some(params) = params {
        obj.insert("params".to_string(), params);
    }
    to_bytes(Value::Object(obj))
}

/// 编码一条 notification（无 `id`，server 不回响应）。`notifications/initialized` 用它。
pub fn encode_notification(method: &str, params: Option<Value>) -> Vec<u8> {
    let mut obj = Map::new();
    obj.insert("jsonrpc".to_string(), json!("2.0"));
    obj.insert("method".to_string(), json!(method));
    if let Some(params) = params {
        obj.insert("params".to_string(), params);
    }
    to_bytes(Value::Object(obj))
}

/// `Value` 序列化不会失败（不含非法浮点/循环引用），`expect` 而不是往上传播错误——
/// 让签名保持 `Vec<u8>` 而不是 `Result`（接口已钉死）。
fn to_bytes(value: Value) -> Vec<u8> {
    serde_json::to_vec(&value).expect("serde_json::Value 序列化不会失败")
}

/// 解析一行 JSON-RPC 响应字节 → [`RpcResponse`]。
///
/// **未知不猜成成功**（本层头号原则）。畸形要落到具体的 [`ProtocolError`] 变体：
/// - 不是 JSON → `NotJson`
/// - 缺 `jsonrpc`/`id`，或 `id` 不是整数 → `NotJsonRpc`
/// - `result` 与 `error` 同时在、或都不在 → `Malformed`
pub fn parse_response(bytes: &[u8]) -> Result<RpcResponse, ProtocolError> {
    let value: Value =
        serde_json::from_slice(bytes).map_err(|e| ProtocolError::NotJson(e.to_string()))?;
    let obj = value
        .as_object()
        .ok_or_else(|| ProtocolError::NotJsonRpc("顶层不是 JSON object".to_string()))?;

    if !obj.contains_key("jsonrpc") {
        return Err(ProtocolError::NotJsonRpc("缺 jsonrpc 字段".to_string()));
    }
    let id = obj
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| ProtocolError::NotJsonRpc("缺 id 字段，或 id 不是非负整数".to_string()))?;

    let has_result = obj.contains_key("result");
    let has_error = obj.contains_key("error");
    match (has_result, has_error) {
        (true, true) => Err(ProtocolError::Malformed(
            "result 与 error 同时存在".to_string(),
        )),
        (false, false) => Err(ProtocolError::Malformed(
            "result 与 error 都不存在".to_string(),
        )),
        (true, false) => Ok(RpcResponse::Result {
            id,
            result: obj["result"].clone(),
        }),
        (false, true) => parse_error_object(id, &obj["error"]),
    }
}

/// 解析 `error` 对象。`code`/`message` 缺失或类型不对 → `Malformed`（信封语义畸形，
/// 跟 `result`/`error` 互斥校验同一类问题，不单独开变体）。
fn parse_error_object(id: u64, error_value: &Value) -> Result<RpcResponse, ProtocolError> {
    let code = error_value
        .get("code")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            ProtocolError::Malformed("error 对象缺 code，或 code 不是整数".to_string())
        })?;
    let message = error_value
        .get("message")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            ProtocolError::Malformed("error 对象缺 message，或 message 不是字符串".to_string())
        })?
        .to_string();
    let data = error_value.get("data").cloned();
    Ok(RpcResponse::Error {
        id,
        error: RpcError {
            code,
            message,
            data,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_request_has_envelope_fields_and_no_newline() {
        let bytes = encode_request(1, "tools/list", None);
        assert!(!bytes.contains(&b'\n'));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], json!("2.0"));
        assert_eq!(v["id"], json!(1));
        assert_eq!(v["method"], json!("tools/list"));
        assert!(v.get("params").is_none(), "params 为 None 时不写字段");
    }

    #[test]
    fn encode_request_with_params_roundtrips() {
        let bytes = encode_request(2, "tools/call", Some(json!({"name": "echo"})));
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["params"], json!({"name": "echo"}));
    }

    #[test]
    fn encode_notification_has_no_id() {
        let bytes = encode_notification("notifications/initialized", None);
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["jsonrpc"], json!("2.0"));
        assert_eq!(v["method"], json!("notifications/initialized"));
        assert!(v.get("id").is_none());
        assert!(v.get("params").is_none());
    }

    /// 红线 11 的精神：同样的输入两次编码逐字节相同。
    #[test]
    fn encode_request_deterministic() {
        let a = encode_request(7, "tools/call", Some(json!({"b": 1, "a": 2})));
        let b = encode_request(7, "tools/call", Some(json!({"a": 2, "b": 1})));
        assert_eq!(a, b, "同 key 集合不同插入顺序，编码必须逐字节相同");
    }

    #[test]
    fn parse_response_result_ok() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert_eq!(
            parse_response(bytes).unwrap(),
            RpcResponse::Result {
                id: 1,
                result: json!({"ok": true})
            }
        );
    }

    #[test]
    fn parse_response_error_ok() {
        let bytes = br#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"not found"}}"#;
        assert_eq!(
            parse_response(bytes).unwrap(),
            RpcResponse::Error {
                id: 2,
                error: RpcError {
                    code: -32601,
                    message: "not found".to_string(),
                    data: None
                }
            }
        );
    }

    #[test]
    fn parse_response_error_with_data() {
        let bytes =
            br#"{"jsonrpc":"2.0","id":2,"error":{"code":-1,"message":"boom","data":{"x":1}}}"#;
        let RpcResponse::Error { error, .. } = parse_response(bytes).unwrap() else {
            panic!("expected Error variant");
        };
        assert_eq!(error.data, Some(json!({"x": 1})));
    }

    #[test]
    fn parse_response_not_json() {
        let err = parse_response(b"not json at all").unwrap_err();
        assert!(matches!(err, ProtocolError::NotJson(_)));
    }

    #[test]
    fn parse_response_missing_id_is_not_jsonrpc() {
        let bytes = br#"{"jsonrpc":"2.0","result":{}}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::NotJsonRpc(_)
        ));
    }

    #[test]
    fn parse_response_non_integer_id_is_not_jsonrpc() {
        let bytes = br#"{"jsonrpc":"2.0","id":"abc","result":{}}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::NotJsonRpc(_)
        ));
    }

    #[test]
    fn parse_response_missing_jsonrpc_field_is_not_jsonrpc() {
        let bytes = br#"{"id":1,"result":{}}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::NotJsonRpc(_)
        ));
    }

    #[test]
    fn parse_response_result_and_error_both_present_is_malformed() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"result":{},"error":{"code":1,"message":"x"}}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::Malformed(_)
        ));
    }

    #[test]
    fn parse_response_neither_result_nor_error_is_malformed() {
        let bytes = br#"{"jsonrpc":"2.0","id":1}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::Malformed(_)
        ));
    }

    #[test]
    fn parse_response_error_object_missing_code_is_malformed() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"error":{"message":"x"}}"#;
        assert!(matches!(
            parse_response(bytes).unwrap_err(),
            ProtocolError::Malformed(_)
        ));
    }

    /// 未知的额外字段忽略不报错（协议向前兼容）。
    #[test]
    fn parse_response_ignores_unknown_extra_fields() {
        let bytes = br#"{"jsonrpc":"2.0","id":1,"result":{},"extra":"whatever"}"#;
        assert!(parse_response(bytes).is_ok());
    }
}
