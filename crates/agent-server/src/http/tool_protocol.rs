//! 远端宿主工具 v2 的 HTTP wire 类型。
//!
//! 这里只有可序列化形状；actor 的认领、epoch 与回执账本不依赖 HTTP。

use serde::{Deserialize, Serialize};

use agent_core::{AgentId, ToolCallId, ToolCallRequest};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolClaimRequest {
    pub(crate) agent: AgentId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) claim_id: String,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolClaimDisposition {
    Claimed,
    AlreadyClaimedByYou,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolClaimResponse {
    pub(crate) disposition: ToolClaimDisposition,
    pub(crate) agent: AgentId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) request: ToolCallRequest,
    pub(crate) revision: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case", tag = "status")]
pub(crate) enum ToolOutcome {
    Succeeded { content: String },
    Failed { error: ToolFailure },
    Cancelled { reason: String },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolFailure {
    pub(crate) code: String,
    pub(crate) message: String,
    pub(crate) retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub(crate) details: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolResultV2Request {
    pub(crate) agent: AgentId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) claim_id: String,
    pub(crate) submission_id: String,
    pub(crate) outcome: ToolOutcome,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolResultDisposition {
    Committed,
    Duplicate,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolTerminalStatus {
    Succeeded,
    Failed,
    Cancelled,
    UnclaimedTimeout,
    OutcomeUnknown,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolResultResponse {
    pub(crate) disposition: ToolResultDisposition,
    pub(crate) terminal_status: ToolTerminalStatus,
    pub(crate) agent: AgentId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) submission_id: String,
    pub(crate) revision: u64,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolCallState {
    PendingUnclaimed,
    Claimed,
    Succeeded,
    Failed,
    Cancelled,
    UnclaimedTimeout,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolTerminalOrigin {
    Host,
    Session,
    Deadline,
}

#[derive(Clone, Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct ToolStatusResponse {
    pub(crate) state: ToolCallState,
    pub(crate) revision: u64,
    pub(crate) retention_floor_revision: Option<u64>,
    pub(crate) agent: AgentId,
    pub(crate) tool_call_id: ToolCallId,
    pub(crate) request: Option<ToolCallRequest>,
    pub(crate) created_at_unix_ms: u64,
    pub(crate) updated_at_unix_ms: u64,
    pub(crate) deadline_at_unix_ms: Option<u64>,
    pub(crate) claimed_by_me: bool,
    pub(crate) submission_id: Option<String>,
    pub(crate) terminal_origin: Option<ToolTerminalOrigin>,
}
