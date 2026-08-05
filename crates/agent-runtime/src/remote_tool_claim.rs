//! Atomic claim transition for one pending remote tool call.

use std::time::{Instant, SystemTime};

use agent_core::Session;

use crate::ctx::RunnerCtx;
use crate::remote_tool_protocol::{
    RemoteToolClaimDecision, RemoteToolClaimGrant, RemoteToolClaimRequest,
    RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};

pub fn claim_remote_tool(
    session: &Session,
    ctx: &mut RunnerCtx,
    request: RemoteToolClaimRequest,
) -> RemoteToolClaimDecision {
    if let Some(receipt) = ctx.remote_tool_receipt(&request.agent, &request.call_id) {
        return RemoteToolClaimDecision::Terminal(receipt.clone());
    }

    let Some(index) =
        ctx.pending_remote_tools.pending.iter().position(|pending| {
            pending.agent == request.agent && pending.call_id == request.call_id
        })
    else {
        return if ctx
            .pending_remote_tools
            .receipts
            .retention_floor_revision()
            .is_some()
        {
            RemoteToolClaimDecision::StatusNotRetained
        } else {
            RemoteToolClaimDecision::UnknownToolCall
        };
    };

    if ctx.pending_remote_tools.pending[index].epoch != session.epoch() {
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
        return RemoteToolClaimDecision::Terminal(receipt);
    }

    match &ctx.pending_remote_tools.pending[index].claim_id {
        Some(claim_id) if claim_id == &request.claim_id => {
            RemoteToolClaimDecision::AlreadyClaimedByYou(RemoteToolClaimGrant {
                request: ctx.pending_remote_tools.pending[index].request.clone(),
                revision: ctx.pending_remote_tools.revision,
            })
        }
        Some(_) => RemoteToolClaimDecision::ClaimedByOther,
        None => {
            let claimed_at = SystemTime::now();
            let deadline_at = claimed_at
                .checked_add(ctx.remote_tool_timeout)
                .unwrap_or(claimed_at);
            let deadline = Instant::now() + ctx.remote_tool_timeout;
            let granted_request = {
                let pending = &mut ctx.pending_remote_tools.pending[index];
                pending.claim_id = Some(request.claim_id);
                pending.claimed_at = Some(claimed_at);
                pending.deadline_at = deadline_at;
                pending.deadline = deadline;
                pending.request.clone()
            };
            ctx.pending_remote_tools.bump_revision();
            let revision = ctx.pending_remote_tools.revision;
            ctx.publish_remote_tool_status();
            RemoteToolClaimDecision::Claimed(RemoteToolClaimGrant {
                request: granted_request,
                revision,
            })
        }
    }
}
