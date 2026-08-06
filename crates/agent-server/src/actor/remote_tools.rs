//! Actor-thread handlers for the remote-tool request/reply protocol.

use tokio::sync::{broadcast, oneshot};

use agent_core::{Failure, Session, TurnStatus};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitRequest, RunnerCtx, claim_remote_tool, submit_remote_tool_result,
};

use crate::event::Frame;

use super::commands;

pub(super) fn claim(
    session: &Session,
    ctx: &mut RunnerCtx,
    request: RemoteToolClaimRequest,
    reply: oneshot::Sender<RemoteToolClaimDecision>,
) {
    let _ = reply.send(claim_remote_tool(session, ctx, request));
}

pub(super) fn submit(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    events: &broadcast::Sender<Frame>,
    request: RemoteToolSubmitRequest,
    reply: oneshot::Sender<RemoteToolSubmitDecision>,
) {
    let status = submit_remote_tool_result(session, ctx, request, |decision| {
        let _ = reply.send(decision);
    });
    match status {
        Ok(Some(TurnStatus::Failed(Failure::Cancelled))) => {
            commands::erase_cancelled_turn(session, ctx, events)
        }
        Ok(_) => {}
        Err(failure) => commands::emit_transient_source_failure(events, failure),
    }
}
