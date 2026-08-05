//! Read-only projection of active remote calls and recent terminal receipts.

use crate::ctx::RunnerCtx;
use crate::remote_tool_protocol::{
    RemoteToolActive, RemoteToolActiveState, RemoteToolStatusSnapshot,
};

impl RunnerCtx {
    pub fn remote_tool_status(&self) -> RemoteToolStatusSnapshot {
        let active = self
            .pending_remote_tools
            .pending
            .iter()
            .map(|pending| RemoteToolActive {
                agent: pending.agent.clone(),
                call_id: pending.call_id.clone(),
                request: pending.request.clone(),
                state: match &pending.claim_id {
                    Some(claim_id) => RemoteToolActiveState::Claimed {
                        claim_id: claim_id.clone(),
                    },
                    None => RemoteToolActiveState::PendingUnclaimed,
                },
                registered_at: pending.registered_at,
                updated_at: pending.claimed_at.unwrap_or(pending.registered_at),
                deadline_at: pending.deadline_at,
            })
            .collect();
        RemoteToolStatusSnapshot {
            revision: self.pending_remote_tools.revision,
            retention_floor_revision: self
                .pending_remote_tools
                .receipts
                .retention_floor_revision(),
            active,
            recent_terminal: self.pending_remote_tools.receipts.snapshot(),
        }
    }

    /// Install the complete protocol-status projection callback. The callback receives the
    /// post-transition snapshot and replaces any previously installed callback.
    pub fn with_remote_tool_status(
        mut self,
        on_change: Box<dyn FnMut(RemoteToolStatusSnapshot)>,
    ) -> Self {
        self.on_remote_tool_status = Some(on_change);
        self
    }

    pub(crate) fn publish_remote_tool_status(&mut self) {
        if self.on_remote_tool_status.is_none() {
            return;
        }
        let snapshot = self.remote_tool_status();
        if let Some(on_change) = self.on_remote_tool_status.as_mut() {
            on_change(snapshot);
        }
    }
}
