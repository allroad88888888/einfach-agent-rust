//! `POST /sessions/:id/tool_claim`: atomically obtain one remote-tool execution grant.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use agent_runtime::{RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolReceipt};

use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::http::tool_protocol::{
    ToolClaimDisposition, ToolClaimRequest, ToolClaimResponse, ToolTerminalStatus,
};
use crate::registry::SessionId;

use super::remote_tool_actor;
use super::remote_tool_validation;

pub(in crate::http) async fn claim(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<ToolClaimRequest>,
) -> Result<Json<ToolClaimResponse>, ApiError> {
    remote_tool_validation::query(body.agent.as_str(), body.tool_call_id.0.as_ref())?;
    remote_tool_validation::claim_id(&body.claim_id)?;

    let handle = state.session_handle(&SessionId::from(id))?;
    let request = RemoteToolClaimRequest {
        agent: body.agent.clone(),
        call_id: body.tool_call_id.clone(),
        claim_id: body.claim_id,
    };
    match remote_tool_actor::claim(&handle, request).await? {
        RemoteToolClaimDecision::Claimed(grant) => Ok(Json(ToolClaimResponse {
            disposition: ToolClaimDisposition::Claimed,
            agent: body.agent,
            tool_call_id: body.tool_call_id,
            request: Some(grant.request),
            revision: grant.revision,
        })),
        RemoteToolClaimDecision::AlreadyClaimedByYou(grant) => Ok(Json(ToolClaimResponse {
            disposition: ToolClaimDisposition::AlreadyClaimedByYou,
            agent: body.agent,
            tool_call_id: body.tool_call_id,
            request: Some(grant.request),
            revision: grant.revision,
        })),
        RemoteToolClaimDecision::ClaimedByOther { revision } => Ok(Json(ToolClaimResponse {
            disposition: ToolClaimDisposition::Ignored,
            agent: body.agent,
            tool_call_id: body.tool_call_id,
            request: None,
            revision,
        })),
        RemoteToolClaimDecision::Terminal(receipt) => Err(terminal_error(&receipt)),
        RemoteToolClaimDecision::StatusNotRetained => Err(ApiError::remote_tool(
            StatusCode::GONE,
            "status_not_retained",
            "这个 tool call 的终态回执已超出保留窗口",
        )),
        RemoteToolClaimDecision::UnknownToolCall => Err(ApiError::remote_tool(
            StatusCode::NOT_FOUND,
            "tool_call_unknown",
            "找不到这个待执行的 tool call",
        )),
    }
}

pub(super) fn terminal_status(receipt: &RemoteToolReceipt) -> ToolTerminalStatus {
    match &receipt.status {
        agent_runtime::RemoteToolTerminalStatus::Succeeded => ToolTerminalStatus::Succeeded,
        agent_runtime::RemoteToolTerminalStatus::Failed => ToolTerminalStatus::Failed,
        agent_runtime::RemoteToolTerminalStatus::Cancelled => ToolTerminalStatus::Cancelled,
        agent_runtime::RemoteToolTerminalStatus::UnclaimedTimeout => {
            ToolTerminalStatus::UnclaimedTimeout
        }
        agent_runtime::RemoteToolTerminalStatus::OutcomeUnknown => {
            ToolTerminalStatus::OutcomeUnknown
        }
    }
}

pub(super) fn terminal_error(receipt: &RemoteToolReceipt) -> ApiError {
    ApiError::remote_tool(
        StatusCode::GONE,
        "tool_call_terminal",
        format!("这个 tool call 已进入 {:?} 终态", terminal_status(receipt)),
    )
}
