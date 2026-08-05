//! Bounds for remote-tool v2 HTTP requests.

use crate::http::error::ApiError;
use crate::http::tool_protocol::{ToolOutcome, ToolResultV2Request};

pub(super) const MAX_TOOL_RESULT_BODY_BYTES: usize = 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 256;

pub(super) fn claim_id(value: &str) -> Result<(), ApiError> {
    identifier("claim_id", value)
}

pub(super) fn query(agent: &str, tool_call_id: &str) -> Result<(), ApiError> {
    identifier("agent", agent)?;
    identifier("tool_call_id", tool_call_id)
}

pub(super) fn result(request: &ToolResultV2Request) -> Result<(), ApiError> {
    query(request.agent.as_str(), request.tool_call_id.0.as_ref())?;
    identifier("claim_id", &request.claim_id)?;
    identifier("submission_id", &request.submission_id)?;
    outcome(&request.outcome)
}

pub(super) fn content(value: &str) -> Result<(), ApiError> {
    if value.len() > MAX_TOOL_RESULT_BODY_BYTES {
        return Err(ApiError::bad_request(format!(
            "tool result content 不能超过 {MAX_TOOL_RESULT_BODY_BYTES} bytes"
        )));
    }
    Ok(())
}

fn identifier(name: &str, value: &str) -> Result<(), ApiError> {
    if value.is_empty() || value.len() > MAX_IDENTIFIER_BYTES {
        return Err(ApiError::bad_request(format!(
            "{name} 必须介于 1 和 {MAX_IDENTIFIER_BYTES} bytes 之间"
        )));
    }
    Ok(())
}

fn outcome(value: &ToolOutcome) -> Result<(), ApiError> {
    match value {
        ToolOutcome::Succeeded { content: value } => content(value),
        ToolOutcome::Failed { error } => {
            content(&error.code)?;
            content(&error.message)?;
            if let Some(details) = &error.details {
                let size = serde_json::to_vec(details)
                    .expect("serde_json::Value serialization cannot fail")
                    .len();
                if size > MAX_TOOL_RESULT_BODY_BYTES {
                    return Err(ApiError::bad_request(format!(
                        "tool failure details 不能超过 {MAX_TOOL_RESULT_BODY_BYTES} bytes"
                    )));
                }
            }
            Ok(())
        }
        ToolOutcome::Cancelled { reason } => content(reason),
    }
}
