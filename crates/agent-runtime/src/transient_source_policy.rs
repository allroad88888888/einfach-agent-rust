//! Exact-name policy and durable placeholders for transient source tools.

use std::sync::Arc;

use agent_core::ToolCallRequest;
use serde_json::{Value, json};

const SOURCE_TOOL_PREFIX: &str = "web:source/";

pub(crate) const SAFE_RESULT: &str = "[transient_source_result_redacted]";
pub(crate) const SAFE_ERROR: &str = "[transient_source_error_redacted]";
pub(crate) const SAFE_CANDIDATE: &str = "[transient_source_candidate_redacted]";
pub(crate) const SAFE_INGRESS_ERROR: &str = "invalid transient source tool batch";

/// 是否为 `web:source/` 前缀的 transient-source 工具名。
///
/// 124：这是唯一允许跨 crate 判定这件事的入口——`SOURCE_TOOL_PREFIX` 本身仍是
/// `agent-runtime` 内部私有常量。`agent-wasm` 的 drain 循环靠这个函数决定
/// 一次远端回传该走 [`crate::resolve_remote_tool_async`]（简单路径，拒绝
/// transient-source）还是 [`crate::submit_remote_tool_result_async`]（唯一认得
/// transient-source 的正门，见 [`crate::claim_remote_tool`] 对这类工具的真入参
/// 解析），不允许在 `agent-wasm` 里重抄一份 `"web:source/"` 字面量——两份前缀
/// 常量哪天被改歪一个，症状是安全策略静默失效（入参/结果照常进历史，不报错）。
pub fn is_transient_source(name: &str) -> bool {
    name
        .strip_prefix(SOURCE_TOOL_PREFIX)
        .is_some_and(|operation| !operation.is_empty())
}

pub(crate) fn placeholder_input() -> Arc<Value> {
    Arc::new(json!({"transient_source": "redacted"}))
}

pub(crate) fn is_placeholder_input(input: &Value) -> bool {
    input == placeholder_input().as_ref()
}

pub(crate) fn sanitize_request(request: &ToolCallRequest) -> ToolCallRequest {
    ToolCallRequest {
        tool: Arc::clone(&request.tool),
        input: placeholder_input(),
        location: request.location,
        reversibility: request.reversibility,
    }
}
