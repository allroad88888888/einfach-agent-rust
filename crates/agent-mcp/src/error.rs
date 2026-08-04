//! 协议层的错误分类。**未知不猜成成功**是本层的头号原则（对齐 `agent-providers` 的
//! `StopReason::Other`：未知 `finish_reason` 不猜成 `EndTurn`）——猜错了宿主会把一条
//! 畸形/失败响应当成有效结果喂进 loop。

/// 解析 JSON-RPC 信封或某个 MCP 方法 result 时的失败。
///
/// 041 的验收要求畸形帧（`id` 缺失、`result` 与 `error` 同时在或都不在）落到明确的
/// `Err`，所以每一类畸形要能被区分断言，而不是糊成一个 `String`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// 根本不是合法 JSON。
    NotJson(String),
    /// 是 JSON 但不是合法的 JSON-RPC 信封（缺 `jsonrpc`/`id`，或 `id` 类型不对）。
    NotJsonRpc(String),
    /// 信封合法但语义畸形：`result` 与 `error` 同时存在、或两者都不存在。
    Malformed(String),
    /// 某个方法的 result 形状不符（如 `tools/list` 的 result 里没有 `tools` 数组）。
    UnexpectedShape(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::NotJson(m) => write!(f, "不是合法 JSON: {m}"),
            ProtocolError::NotJsonRpc(m) => write!(f, "不是合法 JSON-RPC 信封: {m}"),
            ProtocolError::Malformed(m) => write!(f, "JSON-RPC 语义畸形: {m}"),
            ProtocolError::UnexpectedShape(m) => write!(f, "方法 result 形状不符: {m}"),
        }
    }
}

impl std::error::Error for ProtocolError {}
