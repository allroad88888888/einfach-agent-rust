//! `POST /sessions/:id/tool_result`：Web 宿主确认一个先前由 SSE 派发的工具。
//!
//! 此端点只把结果送往该 session 的 actor；真正的安全校验在 actor 持有的
//! `RunnerCtx` 中完成，必须精确匹配仍在等待的 `(agent, call_id)`。所以 HTTP
//! 客户端不能指定 epoch，也不能伪造结果填充任意本地工具调用。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use agent_core::{AgentId, ToolCallId};

use crate::Command;
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::SessionId;

const MAX_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
pub(in crate::http) struct ToolResultRequest {
    agent: AgentId,
    tool_call_id: ToolCallId,
    result: ToolResult,
}

#[derive(Deserialize)]
pub(in crate::http) struct ToolResult {
    content: String,
    #[serde(default)]
    is_error: bool,
}

pub(in crate::http) async fn tool_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToolResultRequest>,
) -> Result<StatusCode, ApiError> {
    if body.result.content.len() > MAX_RESULT_BYTES {
        return Err(ApiError::bad_request(format!(
            "tool result content 不能超过 {MAX_RESULT_BYTES} bytes"
        )));
    }
    state.dispatch(
        &SessionId::from(id),
        Command::RemoteToolResult {
            agent: body.agent,
            call_id: body.tool_call_id,
            content: body.result.content,
            is_error: body.result.is_error,
        },
    )?;
    Ok(StatusCode::ACCEPTED)
}
