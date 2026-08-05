//! Process-local ownership of transient source inputs and outcomes.

use std::sync::Arc;

use agent_core::{AgentId, Epoch, ToolCallId};
use serde_json::Value;

use crate::remote_tool_protocol::RemoteToolSubmitOutcome;

#[derive(Clone)]
pub(crate) struct CapturedSource {
    pub(crate) agent: AgentId,
    pub(crate) epoch: Epoch,
    pub(crate) call_id: ToolCallId,
    pub(crate) tool: Arc<str>,
    pub(crate) input: Arc<Value>,
    /// Provider-native reasoning for the assistant turn that created this batch.
    pub(crate) reasoning: Option<Arc<str>>,
}

pub(crate) struct TransientSourceSecret {
    pub(crate) call_id: ToolCallId,
    pub(crate) tool: Arc<str>,
    pub(crate) input: Arc<Value>,
    pub(crate) outcome: Arc<str>,
    pub(crate) is_error: bool,
}

pub(crate) struct TransientSourceReasoning {
    pub(crate) call_id: ToolCallId,
    pub(crate) tool: Arc<str>,
    pub(crate) reasoning: Option<Arc<str>>,
}

pub(crate) struct TransientSourceReplay {
    pub(crate) current: Vec<TransientSourceSecret>,
    pub(crate) reasoning: Vec<TransientSourceReasoning>,
}

struct PendingPayload {
    input: Arc<Value>,
    outcome: Option<(Arc<str>, bool)>,
}

struct Entry {
    agent: AgentId,
    epoch: Epoch,
    call_id: ToolCallId,
    tool: Arc<str>,
    reasoning: Option<Arc<str>>,
    pending: Option<PendingPayload>,
}

#[derive(Default)]
pub(crate) struct TransientSourceVault {
    entries: Vec<Entry>,
}

impl TransientSourceVault {
    /// Commit a provider batch atomically after the caller has validated every block.
    pub(crate) fn capture_batch(&mut self, batch: Vec<CapturedSource>) -> Result<(), ()> {
        let duplicate = batch.iter().enumerate().any(|(index, item)| {
            batch[..index].iter().any(|earlier| same_key(earlier, item))
                || self.entries.iter().any(|entry| {
                    entry.agent == item.agent
                        && entry.epoch == item.epoch
                        && entry.call_id == item.call_id
                })
        });
        if duplicate {
            return Err(());
        }
        self.entries.extend(batch.into_iter().map(Entry::from));
        Ok(())
    }

    pub(crate) fn raw_input(
        &self,
        agent: &AgentId,
        epoch: Epoch,
        call_id: &ToolCallId,
        tool: &str,
    ) -> Option<Arc<Value>> {
        self.find(agent, epoch, call_id)
            .filter(|entry| &*entry.tool == tool)
            .and_then(|entry| entry.pending.as_ref())
            .map(|pending| Arc::clone(&pending.input))
    }

    pub(crate) fn record_outcome(
        &mut self,
        agent: &AgentId,
        epoch: Epoch,
        call_id: &ToolCallId,
        outcome: &RemoteToolSubmitOutcome,
    ) -> Result<(), ()> {
        let Some(entry) = self.find_mut(agent, epoch, call_id) else {
            return Err(());
        };
        let Some(pending) = entry.pending.as_mut() else {
            return Err(());
        };
        if pending.outcome.is_some() {
            return Err(());
        }
        pending.outcome = Some(canonical_outcome(outcome));
        Ok(())
    }

    /// Consume only the current raw payloads while retaining per-call reasoning for the chain.
    pub(crate) fn take_ready_hop(
        &mut self,
        agent: &AgentId,
        epoch: Epoch,
        expected: &[ToolCallId],
    ) -> Result<Option<TransientSourceReplay>, ()> {
        let chain: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.agent == *agent && entry.epoch == epoch)
            .map(|(index, _)| index)
            .collect();
        if chain.is_empty() {
            return Ok(None);
        }
        let current: Vec<usize> = chain
            .iter()
            .copied()
            .filter(|index| self.entries[*index].pending.is_some())
            .collect();
        if current.is_empty()
            || !same_call_ids(
                expected,
                current.iter().map(|index| &self.entries[*index].call_id),
            )
            || current.iter().any(|index| {
                self.entries[*index]
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.outcome.is_none())
            })
        {
            return Err(());
        }
        let reasoning = chain
            .iter()
            .map(|index| {
                let entry = &self.entries[*index];
                TransientSourceReasoning {
                    call_id: entry.call_id.clone(),
                    tool: Arc::clone(&entry.tool),
                    reasoning: entry.reasoning.clone(),
                }
            })
            .collect();
        let mut secrets = Vec::with_capacity(current.len());
        for index in current {
            let entry = &mut self.entries[index];
            let pending = entry.pending.take().expect("current entry checked above");
            let (outcome, is_error) = pending.outcome.expect("ready entry checked above");
            secrets.push(TransientSourceSecret {
                call_id: entry.call_id.clone(),
                tool: Arc::clone(&entry.tool),
                input: pending.input,
                outcome,
                is_error,
            });
        }
        Ok(Some(TransientSourceReplay {
            current: secrets,
            reasoning,
        }))
    }

    pub(crate) fn purge_call(&mut self, agent: &AgentId, call_id: &ToolCallId) {
        self.entries
            .retain(|entry| entry.agent != *agent || entry.call_id != *call_id);
    }

    pub(crate) fn purge_agent_epoch(&mut self, agent: &AgentId, epoch: Epoch) {
        self.entries
            .retain(|entry| entry.agent != *agent || entry.epoch != epoch);
    }

    pub(crate) fn purge_agent(&mut self, agent: &AgentId) {
        self.entries.retain(|entry| entry.agent != *agent);
    }

    pub(crate) fn purge_all(&mut self) {
        self.entries.clear();
    }

    fn find(&self, agent: &AgentId, epoch: Epoch, call_id: &ToolCallId) -> Option<&Entry> {
        self.entries.iter().find(|entry| {
            entry.agent == *agent && entry.epoch == epoch && entry.call_id == *call_id
        })
    }

    fn find_mut(
        &mut self,
        agent: &AgentId,
        epoch: Epoch,
        call_id: &ToolCallId,
    ) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| {
            entry.agent == *agent && entry.epoch == epoch && entry.call_id == *call_id
        })
    }
}

impl From<CapturedSource> for Entry {
    fn from(captured: CapturedSource) -> Self {
        Self {
            agent: captured.agent,
            epoch: captured.epoch,
            call_id: captured.call_id,
            tool: captured.tool,
            reasoning: captured.reasoning,
            pending: Some(PendingPayload {
                input: captured.input,
                outcome: None,
            }),
        }
    }
}

fn same_key(left: &CapturedSource, right: &CapturedSource) -> bool {
    left.agent == right.agent && left.epoch == right.epoch && left.call_id == right.call_id
}

fn same_call_ids<'a>(
    expected: &[ToolCallId],
    actual: impl Iterator<Item = &'a ToolCallId>,
) -> bool {
    let actual: Vec<_> = actual.collect();
    expected.len() == actual.len()
        && expected
            .iter()
            .all(|id| actual.iter().filter(|actual| **actual == id).count() == 1)
}

fn canonical_outcome(outcome: &RemoteToolSubmitOutcome) -> (Arc<str>, bool) {
    let value = match outcome {
        RemoteToolSubmitOutcome::Succeeded { content } => {
            return (Arc::from(content.as_str()), false);
        }
        RemoteToolSubmitOutcome::Failed { error } => serde_json::json!({
            "status": "failed",
            "error": {
                "code": error.code,
                "message": error.message,
                "retryable": error.retryable,
                "details": error.details,
            }
        }),
        RemoteToolSubmitOutcome::Cancelled { reason } => serde_json::json!({
            "status": "cancelled",
            "reason": reason,
        }),
    };
    let encoded = serde_json::to_string(&value).expect("JSON value serialization cannot fail");
    (Arc::from(encoded), true)
}
