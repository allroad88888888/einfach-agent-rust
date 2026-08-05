//! Transport-neutral values for claiming and completing a remote tool call.

use std::time::SystemTime;

use agent_core::{AgentId, ToolCallId, ToolCallRequest};

use crate::remote_tool_digest::sha256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteToolTerminalStatus {
    Succeeded,
    Failed,
    Cancelled,
    UnclaimedTimeout,
    OutcomeUnknown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteToolTerminalOrigin {
    Host,
    Session,
    Deadline,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteToolActiveState {
    PendingUnclaimed,
    Claimed { claim_id: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteToolActive {
    pub agent: AgentId,
    pub call_id: ToolCallId,
    pub request: ToolCallRequest,
    pub state: RemoteToolActiveState,
    pub registered_at: SystemTime,
    pub updated_at: SystemTime,
    pub deadline_at: SystemTime,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteToolReceipt {
    pub agent: AgentId,
    pub call_id: ToolCallId,
    pub revision: u64,
    pub status: RemoteToolTerminalStatus,
    pub origin: RemoteToolTerminalOrigin,
    pub submission_id: Option<String>,
    pub payload_digest: Option<[u8; 32]>,
    pub payload_len: Option<usize>,
    pub created_at: SystemTime,
    pub terminal_at: SystemTime,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemoteToolStatusSnapshot {
    pub revision: u64,
    pub retention_floor_revision: Option<u64>,
    pub active: Vec<RemoteToolActive>,
    pub recent_terminal: Vec<RemoteToolReceipt>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteToolClaimGrant {
    pub request: ToolCallRequest,
    pub revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteToolClaimRequest {
    pub agent: AgentId,
    pub call_id: ToolCallId,
    pub claim_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteToolClaimDecision {
    Claimed(RemoteToolClaimGrant),
    AlreadyClaimedByYou(RemoteToolClaimGrant),
    ClaimedByOther(RemoteToolClaimGrant),
    Terminal(RemoteToolReceipt),
    StatusNotRetained,
    UnknownToolCall,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteToolSubmitRequest {
    pub agent: AgentId,
    pub call_id: ToolCallId,
    pub claim_id: String,
    pub submission_id: String,
    pub outcome: RemoteToolSubmitOutcome,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RemoteToolSubmitOutcome {
    Succeeded { content: String },
    Failed { error: RemoteToolFailure },
    Cancelled { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteToolFailure {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteToolSubmitDecision {
    Committed(RemoteToolReceipt),
    Duplicate(RemoteToolReceipt),
    Conflict(RemoteToolReceipt),
    ClaimRequired,
    ClaimedByOther,
    Terminal(RemoteToolReceipt),
    StatusNotRetained,
    UnknownToolCall,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PayloadFingerprint {
    pub(crate) digest: [u8; 32],
    pub(crate) len: usize,
}

impl RemoteToolSubmitOutcome {
    pub(crate) fn terminal_status(&self) -> RemoteToolTerminalStatus {
        match self {
            Self::Succeeded { .. } => RemoteToolTerminalStatus::Succeeded,
            Self::Failed { .. } => RemoteToolTerminalStatus::Failed,
            Self::Cancelled { .. } => RemoteToolTerminalStatus::Cancelled,
        }
    }

    pub(crate) fn fingerprint(&self) -> PayloadFingerprint {
        let mut canonical = Vec::new();
        match self {
            Self::Succeeded { content } => {
                canonical.push(0);
                push_bytes(&mut canonical, content.as_bytes());
            }
            Self::Failed { error } => {
                canonical.push(1);
                push_bytes(&mut canonical, error.code.as_bytes());
                push_bytes(&mut canonical, error.message.as_bytes());
                canonical.push(u8::from(error.retryable));
                match &error.details {
                    Some(details) => {
                        canonical.push(1);
                        let bytes = serde_json::to_vec(details)
                            .expect("serde_json::Value serialization cannot fail");
                        push_bytes(&mut canonical, &bytes);
                    }
                    None => canonical.push(0),
                }
            }
            Self::Cancelled { reason } => {
                canonical.push(2);
                push_bytes(&mut canonical, reason.as_bytes());
            }
        }
        PayloadFingerprint {
            digest: sha256(&canonical),
            len: canonical.len(),
        }
    }
}

fn push_bytes(output: &mut Vec<u8>, bytes: &[u8]) {
    output.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    output.extend_from_slice(bytes);
}
