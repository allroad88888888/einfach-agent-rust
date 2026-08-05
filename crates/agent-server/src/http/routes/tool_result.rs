//! `POST /sessions/:id/tool_result`：Web 宿主确认一个先前由 SSE 派发的工具。
//!
//! 此端点只把结果送往该 session 的 actor；真正的安全校验在 actor 持有的
//! `RunnerCtx` 中完成，必须精确匹配仍在等待的 `(agent, call_id)`。所以 HTTP
//! 客户端不能指定 epoch，也不能伪造结果填充任意本地工具调用。

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

use agent_core::{AgentId, ToolCallId};
use agent_runtime::{
    RemoteToolFailure, RemoteToolSubmitDecision, RemoteToolSubmitOutcome, RemoteToolSubmitRequest,
};

use crate::Command;
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::http::tool_protocol::{
    ToolOutcome, ToolResultDisposition, ToolResultResponse, ToolResultV2Request,
};
use crate::registry::SessionId;

use super::remote_tool_actor;
use super::remote_tool_validation;
use super::tool_claim::{terminal_error, terminal_status};

#[derive(Deserialize)]
#[serde(untagged)]
pub(in crate::http) enum ToolResultRequest {
    V2(ToolResultV2Request),
    V1(LegacyToolResultRequest),
}

#[derive(Deserialize)]
pub(in crate::http) struct LegacyToolResultRequest {
    agent: AgentId,
    tool_call_id: ToolCallId,
    result: LegacyToolResult,
}

#[derive(Deserialize)]
pub(in crate::http) struct LegacyToolResult {
    content: String,
    #[serde(default)]
    is_error: bool,
}

pub(in crate::http) async fn tool_result(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToolResultRequest>,
) -> Result<Response, ApiError> {
    match body {
        ToolResultRequest::V1(body) => legacy_result(state, id, body).await,
        ToolResultRequest::V2(body) => result_v2(state, id, body).await,
    }
}

async fn legacy_result(
    state: AppState,
    id: String,
    body: LegacyToolResultRequest,
) -> Result<Response, ApiError> {
    remote_tool_validation::query(body.agent.as_str(), body.tool_call_id.0.as_ref())?;
    remote_tool_validation::content(&body.result.content)?;
    state.dispatch(
        &SessionId::from(id),
        Command::RemoteToolResult {
            agent: body.agent,
            call_id: body.tool_call_id,
            content: body.result.content,
            is_error: body.result.is_error,
        },
    )?;
    let mut response = StatusCode::ACCEPTED.into_response();
    response
        .headers_mut()
        .insert("deprecation", HeaderValue::from_static("true"));
    response.headers_mut().insert(
        "x-remote-tool-protocol-deprecated",
        HeaderValue::from_static("v1"),
    );
    Ok(response)
}

async fn result_v2(
    state: AppState,
    id: String,
    body: ToolResultV2Request,
) -> Result<Response, ApiError> {
    remote_tool_validation::result(&body)?;
    let handle = state.session_handle(&SessionId::from(id))?;
    let request = RemoteToolSubmitRequest {
        agent: body.agent.clone(),
        call_id: body.tool_call_id.clone(),
        claim_id: body.claim_id,
        submission_id: body.submission_id.clone(),
        outcome: into_actor_outcome(body.outcome),
    };
    let response = match remote_tool_actor::submit(&handle, request).await? {
        RemoteToolSubmitDecision::Committed(receipt) => ToolResultResponse {
            disposition: ToolResultDisposition::Committed,
            terminal_status: terminal_status(&receipt),
            agent: body.agent,
            tool_call_id: body.tool_call_id,
            submission_id: body.submission_id,
            revision: receipt.revision,
        },
        RemoteToolSubmitDecision::Duplicate(receipt) => ToolResultResponse {
            disposition: ToolResultDisposition::Duplicate,
            terminal_status: terminal_status(&receipt),
            agent: body.agent,
            tool_call_id: body.tool_call_id,
            submission_id: body.submission_id,
            revision: receipt.revision,
        },
        RemoteToolSubmitDecision::Conflict(_) => {
            return Err(ApiError::remote_tool(
                StatusCode::CONFLICT,
                "result_conflict",
                "同一 submission_id 的回传内容不一致，或这个认领已提交过另一份结果",
            ));
        }
        RemoteToolSubmitDecision::ClaimRequired => {
            return Err(ApiError::remote_tool(
                StatusCode::CONFLICT,
                "tool_claim_required",
                "提交 tool result 前必须先成功认领该调用",
            ));
        }
        RemoteToolSubmitDecision::ClaimedByOther => {
            return Err(ApiError::remote_tool(
                StatusCode::CONFLICT,
                "tool_claimed_by_other",
                "这个 tool call 已由另一位宿主认领",
            ));
        }
        RemoteToolSubmitDecision::Terminal(receipt) => return Err(terminal_error(&receipt)),
        RemoteToolSubmitDecision::StatusNotRetained => {
            return Err(ApiError::remote_tool(
                StatusCode::GONE,
                "status_not_retained",
                "这个 tool call 的终态回执已超出保留窗口",
            ));
        }
        RemoteToolSubmitDecision::UnknownToolCall => {
            return Err(ApiError::remote_tool(
                StatusCode::NOT_FOUND,
                "tool_call_unknown",
                "找不到这个待执行的 tool call",
            ));
        }
    };
    Ok(Json(response).into_response())
}

fn into_actor_outcome(outcome: ToolOutcome) -> RemoteToolSubmitOutcome {
    match outcome {
        ToolOutcome::Succeeded { content } => RemoteToolSubmitOutcome::Succeeded { content },
        ToolOutcome::Failed { error } => RemoteToolSubmitOutcome::Failed {
            error: RemoteToolFailure {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
                details: error.details,
            },
        },
        ToolOutcome::Cancelled { reason } => RemoteToolSubmitOutcome::Cancelled { reason },
    }
}
