//! Commit a claimed remote tool outcome and acknowledge it before continuation IO starts.

use std::sync::Arc;

use agent_core::{Event, Session, TurnStatus};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::remote_tool_protocol::{
    PayloadFingerprint, RemoteToolReceipt, RemoteToolSubmitDecision, RemoteToolSubmitOutcome,
    RemoteToolSubmitRequest, RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};
use crate::runner;

/// Submit an outcome through the epoch gate. `acknowledge` is invoked exactly once.  On a new
/// terminal result it runs after the core event is persisted and before any resulting provider
/// effect is dispatched; duplicate and rejection decisions run without advancing the pump.
pub fn submit_remote_tool_result(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    request: RemoteToolSubmitRequest,
    acknowledge: impl FnOnce(RemoteToolSubmitDecision),
) -> Option<TurnStatus> {
    let fingerprint = request.outcome.fingerprint();
    if let Some(receipt) = ctx
        .remote_tool_receipt(&request.agent, &request.call_id)
        .cloned()
    {
        acknowledge(replay_decision(&receipt, &request, &fingerprint));
        return None;
    }

    let Some(pending) = ctx
        .pending_remote_tools
        .pending
        .iter()
        .find(|pending| pending.agent == request.agent && pending.call_id == request.call_id)
    else {
        let decision = if ctx
            .pending_remote_tools
            .receipts
            .retention_floor_revision()
            .is_some()
        {
            RemoteToolSubmitDecision::StatusNotRetained
        } else {
            RemoteToolSubmitDecision::UnknownToolCall
        };
        acknowledge(decision);
        return None;
    };

    if pending.epoch != session.epoch() {
        let pending = ctx
            .take_remote_tool(&request.agent, &request.call_id)
            .expect("pending remote tool was found above");
        let receipt = ctx.record_remote_tool_terminal(
            &pending,
            RemoteToolTerminalStatus::Cancelled,
            RemoteToolTerminalOrigin::Session,
            None,
            None,
        );
        acknowledge(RemoteToolSubmitDecision::Terminal(receipt));
        return None;
    }
    match pending.claim_id.as_deref() {
        None => {
            acknowledge(RemoteToolSubmitDecision::ClaimRequired);
            return None;
        }
        Some(claim_id) if claim_id != request.claim_id => {
            acknowledge(RemoteToolSubmitDecision::ClaimedByOther);
            return None;
        }
        Some(_) => {}
    }

    let pending = ctx
        .take_remote_tool(&request.agent, &request.call_id)
        .expect("validated pending remote tool must still exist on the actor thread");
    let status = request.outcome.terminal_status();
    let (event, output_len, is_error) = outcome_event(&pending, &request.outcome);
    let submission_id = request.submission_id;
    let event_agent = pending.agent.clone();
    let event_call = pending.call_id.clone();
    let event_tool = pending.request.tool.clone();

    Some(runner::resume_after_first_commit(
        session,
        ctx,
        event,
        move |ctx| {
            let receipt = ctx.record_remote_tool_terminal(
                &pending,
                status,
                RemoteToolTerminalOrigin::Host,
                Some(submission_id),
                Some(fingerprint),
            );
            acknowledge(RemoteToolSubmitDecision::Committed(receipt));
            ctx.emit(
                &event_agent,
                RunnerEvent::ToolExecuted {
                    call_id: event_call,
                    tool: event_tool,
                    output_len,
                    is_error,
                },
            );
        },
    ))
}

fn replay_decision(
    receipt: &RemoteToolReceipt,
    request: &RemoteToolSubmitRequest,
    fingerprint: &PayloadFingerprint,
) -> RemoteToolSubmitDecision {
    match receipt.submission_id.as_deref() {
        Some(id)
            if id == request.submission_id
                && receipt.payload_digest == Some(fingerprint.digest)
                && receipt.payload_len == Some(fingerprint.len) =>
        {
            RemoteToolSubmitDecision::Duplicate(receipt.clone())
        }
        Some(_) => RemoteToolSubmitDecision::Conflict(receipt.clone()),
        None => RemoteToolSubmitDecision::Terminal(receipt.clone()),
    }
}

fn outcome_event(
    pending: &crate::ctx_remote_tools::PendingRemoteTool,
    outcome: &RemoteToolSubmitOutcome,
) -> (Event, usize, bool) {
    let (body, is_error) = match outcome {
        RemoteToolSubmitOutcome::Succeeded { content } => (content.clone(), false),
        RemoteToolSubmitOutcome::Failed { error } => (
            format!(
                "[remote_tool_failed] code={} retryable={} message={}",
                error.code, error.retryable, error.message
            ),
            true,
        ),
        RemoteToolSubmitOutcome::Cancelled { reason } => {
            (format!("[remote_tool_cancelled] reason={reason}"), true)
        }
    };
    let output_len = body.len();
    let event = if is_error {
        Event::ToolFailed {
            agent: pending.agent.clone(),
            epoch: pending.epoch,
            call_id: pending.call_id.clone(),
            error: Arc::from(body),
        }
    } else {
        Event::ToolResult {
            agent: pending.agent.clone(),
            epoch: pending.epoch,
            call_id: pending.call_id.clone(),
            content: Arc::from(body),
        }
    };
    (event, output_len, is_error)
}
