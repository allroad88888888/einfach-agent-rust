//! Hard-bounded terminal receipts for recent remote tool calls.

use std::collections::VecDeque;
use std::time::SystemTime;

use agent_core::{AgentId, ToolCallId};

use crate::ctx::RunnerCtx;
use crate::ctx_remote_tools::{PendingRemoteTool, PendingRemoteTools};
use crate::remote_tool_protocol::{
    PayloadFingerprint, RemoteToolReceipt, RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};

pub const REMOTE_TOOL_RECEIPT_CAP: usize = 256;

pub(crate) struct RemoteToolReceipts {
    entries: VecDeque<RemoteToolReceipt>,
    cap: usize,
    retention_floor_revision: Option<u64>,
}

impl Default for RemoteToolReceipts {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            cap: REMOTE_TOOL_RECEIPT_CAP,
            retention_floor_revision: None,
        }
    }
}

impl RemoteToolReceipts {
    pub(crate) fn get(&self, agent: &AgentId, call_id: &ToolCallId) -> Option<&RemoteToolReceipt> {
        self.entries
            .iter()
            .rev()
            .find(|receipt| &receipt.agent == agent && &receipt.call_id == call_id)
    }

    pub(crate) fn push(&mut self, receipt: RemoteToolReceipt) {
        if self.entries.len() == self.cap {
            self.evict_oldest();
        }
        self.entries.push_back(receipt);
        debug_assert!(self.entries.len() <= self.cap);
    }

    pub(crate) fn snapshot(&self) -> Vec<RemoteToolReceipt> {
        self.entries.iter().cloned().collect()
    }

    pub(crate) fn retention_floor_revision(&self) -> Option<u64> {
        self.retention_floor_revision
    }

    pub(crate) fn set_cap(&mut self, cap: usize) {
        self.cap = cap.clamp(1, REMOTE_TOOL_RECEIPT_CAP);
        while self.entries.len() > self.cap {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        let Some(evicted) = self.entries.pop_front() else {
            return;
        };
        self.retention_floor_revision = Some(
            evicted
                .revision
                .checked_add(1)
                .expect("remote tool receipt revision exhausted"),
        );
    }
}

impl RunnerCtx {
    pub(crate) fn record_remote_tool_terminal(
        &mut self,
        pending: &PendingRemoteTool,
        status: RemoteToolTerminalStatus,
        origin: RemoteToolTerminalOrigin,
        submission_id: Option<String>,
        fingerprint: Option<PayloadFingerprint>,
    ) -> RemoteToolReceipt {
        let (payload_digest, payload_len) = fingerprint
            .map(|fingerprint| (Some(fingerprint.digest), Some(fingerprint.len)))
            .unwrap_or((None, None));
        self.pending_remote_tools.bump_revision();
        let revision = self.pending_remote_tools.revision;
        let receipt = RemoteToolReceipt {
            agent: pending.agent.clone(),
            call_id: pending.call_id.clone(),
            revision,
            status,
            origin,
            submission_id,
            payload_digest,
            payload_len,
            created_at: pending.registered_at,
            terminal_at: SystemTime::now(),
        };
        self.pending_remote_tools.receipts.push(receipt.clone());
        self.publish_remote_tool_status();
        receipt
    }

    pub(crate) fn remote_tool_receipt(
        &self,
        agent: &AgentId,
        call_id: &ToolCallId,
    ) -> Option<&RemoteToolReceipt> {
        self.pending_remote_tools.receipts.get(agent, call_id)
    }

    /// Lower the receipt cap for deterministic tests. Values above the production hard cap are
    /// clamped, so no caller can turn the lightweight ledger into unbounded storage.
    pub fn with_remote_tool_receipt_cap(mut self, cap: usize) -> Self {
        self.pending_remote_tools.receipts.set_cap(cap);
        self
    }
}

impl PendingRemoteTools {
    pub(crate) fn bump_revision(&mut self) {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("remote tool protocol revision exhausted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_never_exceeds_its_hard_cap() {
        let mut ledger = RemoteToolReceipts::default();
        for index in 0..(REMOTE_TOOL_RECEIPT_CAP + 3) {
            ledger.push(RemoteToolReceipt {
                agent: AgentId::root(),
                call_id: ToolCallId(index.to_string().into()),
                revision: index as u64,
                status: RemoteToolTerminalStatus::Succeeded,
                origin: RemoteToolTerminalOrigin::Host,
                submission_id: None,
                payload_digest: None,
                payload_len: None,
                created_at: SystemTime::UNIX_EPOCH,
                terminal_at: SystemTime::UNIX_EPOCH,
            });
        }
        assert_eq!(ledger.entries.len(), REMOTE_TOOL_RECEIPT_CAP);
        assert_eq!(ledger.entries.front().unwrap().call_id.0.as_ref(), "3");
    }
}
