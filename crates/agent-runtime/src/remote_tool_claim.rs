//! Atomic claim transition for one pending remote tool call.

use std::sync::Arc;

use agent_core::Session;
// 114b：`Instant`/`SystemTime::now()` panic 在 wasm32-unknown-unknown 上，垫
// `web-time`（native 目标下就是 `std::time` 里那两个类型本尊，行为不变）。
use web_time::{Instant, SystemTime};

use crate::ctx::RunnerCtx;
use crate::remote_tool_protocol::{
    RemoteToolClaimDecision, RemoteToolClaimGrant, RemoteToolClaimRequest,
    RemoteToolTerminalOrigin, RemoteToolTerminalStatus,
};
use crate::transient_source_policy::is_transient_source;

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
        ctx.transient_sources
            .purge_call(&pending.agent, &pending.call_id);
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
        Some(claim_id) if claim_id == &request.claim_id => grant_or_fail_closed(
            session,
            ctx,
            index,
            RemoteToolClaimDecision::AlreadyClaimedByYou,
        ),
        Some(_) => RemoteToolClaimDecision::ClaimedByOther {
            revision: ctx.pending_remote_tools.revision,
        },
        None => {
            let claimed_at = SystemTime::now();
            let deadline_at = claimed_at
                .checked_add(ctx.remote_tool_timeout)
                .unwrap_or(claimed_at);
            let deadline = Instant::now() + ctx.remote_tool_timeout;
            let granted_request = match request_for_grant(session, ctx, index) {
                Some(request) => request,
                None => return fail_closed(ctx, index),
            };
            {
                let pending = &mut ctx.pending_remote_tools.pending[index];
                pending.claim_id = Some(request.claim_id);
                pending.claimed_at = Some(claimed_at);
                pending.deadline_at = deadline_at;
                pending.deadline = deadline;
            }
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

fn grant_or_fail_closed(
    session: &Session,
    ctx: &mut RunnerCtx,
    index: usize,
    wrap: fn(RemoteToolClaimGrant) -> RemoteToolClaimDecision,
) -> RemoteToolClaimDecision {
    let Some(request) = request_for_grant(session, ctx, index) else {
        return fail_closed(ctx, index);
    };
    wrap(RemoteToolClaimGrant {
        request,
        revision: ctx.pending_remote_tools.revision,
    })
}

fn request_for_grant(
    session: &Session,
    ctx: &RunnerCtx,
    index: usize,
) -> Option<agent_core::ToolCallRequest> {
    let pending = &ctx.pending_remote_tools.pending[index];
    if !is_transient_source(&pending.request.tool) {
        return Some(pending.request.clone());
    }
    let input = ctx.transient_sources.raw_input(
        &pending.agent,
        session.epoch(),
        &pending.call_id,
        &pending.request.tool,
    )?;
    Some(agent_core::ToolCallRequest {
        tool: Arc::clone(&pending.request.tool),
        input,
        location: pending.request.location,
        reversibility: pending.request.reversibility,
    })
}

fn fail_closed(ctx: &mut RunnerCtx, index: usize) -> RemoteToolClaimDecision {
    let agent = ctx.pending_remote_tools.pending[index].agent.clone();
    let call_id = ctx.pending_remote_tools.pending[index].call_id.clone();
    let pending = ctx
        .take_remote_tool(&agent, &call_id)
        .expect("pending source tool was found above");
    ctx.transient_sources
        .purge_call(&pending.agent, &pending.call_id);
    let receipt = ctx.record_remote_tool_terminal(
        &pending,
        RemoteToolTerminalStatus::Cancelled,
        RemoteToolTerminalOrigin::Session,
        None,
        None,
    );
    RemoteToolClaimDecision::Terminal(receipt)
}
