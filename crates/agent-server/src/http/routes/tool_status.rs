//! `GET /sessions/:id/tool_status`: one remote tool call's current protocol status.

use std::time::SystemTime;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;

use agent_core::{AgentId, ToolCallId};
use agent_runtime::{
    RemoteToolActive, RemoteToolActiveState, RemoteToolReceipt, RemoteToolStatusSnapshot,
    RemoteToolTerminalOrigin,
};

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::http::tool_protocol::{
    ToolCallState, ToolStatusResponse, ToolTerminalOrigin, ToolTerminalStatus,
};
use crate::registry::SessionId;

use super::remote_tool_actor;
use super::remote_tool_validation;
use super::tool_claim::terminal_status;

#[derive(Deserialize)]
pub(in crate::http) struct ToolStatusQuery {
    agent: AgentId,
    tool_call_id: ToolCallId,
}

pub(in crate::http) async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Query(query): Query<ToolStatusQuery>,
) -> Result<Json<ToolStatusResponse>, ApiError> {
    remote_tool_validation::query(query.agent.as_str(), query.tool_call_id.0.as_ref())?;
    let claim_id = header_claim_id(&headers)?;
    let handle = state.session_handle(&SessionId::from(id))?;
    let snapshot = remote_tool_actor::status(&handle);
    let response = project_status(
        snapshot,
        query.agent,
        query.tool_call_id,
        claim_id.as_deref(),
    )?;
    Ok(Json(response))
}

fn header_claim_id(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    let Some(value) = headers.get("x-tool-claim-id") else {
        return Ok(None);
    };
    let value = value
        .to_str()
        .map_err(|_| ApiError::bad_request("X-Tool-Claim-Id 必须是合法文本"))?;
    remote_tool_validation::claim_id(value)?;
    Ok(Some(value.to_owned()))
}

fn project_status(
    snapshot: RemoteToolStatusSnapshot,
    agent: AgentId,
    tool_call_id: ToolCallId,
    claim_id: Option<&str>,
) -> Result<ToolStatusResponse, ApiError> {
    if let Some(active) = snapshot
        .active
        .iter()
        .find(|active| active.agent == agent && active.call_id == tool_call_id)
    {
        return Ok(active_status(snapshot.revision, active, claim_id));
    }
    if let Some(receipt) = snapshot
        .recent_terminal
        .iter()
        .find(|receipt| receipt.agent == agent && receipt.call_id == tool_call_id)
    {
        return Ok(receipt_status(snapshot.retention_floor_revision, receipt));
    }
    if snapshot.retention_floor_revision.is_some() {
        return Err(ApiError::remote_tool(
            axum::http::StatusCode::GONE,
            "status_not_retained",
            "这个 tool call 的终态回执已超出保留窗口",
        ));
    }
    Err(ApiError::remote_tool(
        axum::http::StatusCode::NOT_FOUND,
        "tool_call_unknown",
        "找不到这个 tool call",
    ))
}

fn active_status(
    revision: u64,
    active: &RemoteToolActive,
    claim_id: Option<&str>,
) -> ToolStatusResponse {
    let (state, claimed_by_me) = match &active.state {
        RemoteToolActiveState::PendingUnclaimed => (ToolCallState::PendingUnclaimed, false),
        RemoteToolActiveState::Claimed { claim_id: owner } => {
            (ToolCallState::Claimed, claim_id == Some(owner.as_str()))
        }
    };
    ToolStatusResponse {
        state,
        revision,
        retention_floor_revision: None,
        agent: active.agent.clone(),
        tool_call_id: active.call_id.clone(),
        request: Some(active.request.clone()),
        created_at_unix_ms: unix_ms(active.registered_at),
        updated_at_unix_ms: unix_ms(active.updated_at),
        deadline_at_unix_ms: Some(unix_ms(active.deadline_at)),
        claimed_by_me,
        submission_id: None,
        terminal_origin: None,
    }
}

fn receipt_status(
    retention_floor_revision: Option<u64>,
    receipt: &RemoteToolReceipt,
) -> ToolStatusResponse {
    ToolStatusResponse {
        state: terminal_state(terminal_status(receipt)),
        revision: receipt.revision,
        retention_floor_revision,
        agent: receipt.agent.clone(),
        tool_call_id: receipt.call_id.clone(),
        request: None,
        created_at_unix_ms: unix_ms(receipt.created_at),
        updated_at_unix_ms: unix_ms(receipt.terminal_at),
        deadline_at_unix_ms: None,
        claimed_by_me: false,
        submission_id: receipt.submission_id.clone(),
        terminal_origin: Some(origin(receipt.origin.clone())),
    }
}

fn terminal_state(status: ToolTerminalStatus) -> ToolCallState {
    match status {
        ToolTerminalStatus::Succeeded => ToolCallState::Succeeded,
        ToolTerminalStatus::Failed => ToolCallState::Failed,
        ToolTerminalStatus::Cancelled => ToolCallState::Cancelled,
        ToolTerminalStatus::UnclaimedTimeout => ToolCallState::UnclaimedTimeout,
        ToolTerminalStatus::OutcomeUnknown => ToolCallState::OutcomeUnknown,
    }
}

fn origin(origin: RemoteToolTerminalOrigin) -> ToolTerminalOrigin {
    match origin {
        RemoteToolTerminalOrigin::Host => ToolTerminalOrigin::Host,
        RemoteToolTerminalOrigin::Session => ToolTerminalOrigin::Session,
        RemoteToolTerminalOrigin::Deadline => ToolTerminalOrigin::Deadline,
    }
}

fn unix_ms(time: SystemTime) -> u64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
