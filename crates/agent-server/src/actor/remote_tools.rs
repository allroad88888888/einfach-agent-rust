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
    // 116：跟 `commands::handle_input` 同一座临时桥（那边的顶部注释有完整理由）——
    // `submit_remote_tool_result` 变成 `async fn` 之后，actor 线程用
    // `agent_runtime::block_on` 把它跑到底。
    let status = agent_runtime::block_on(submit_remote_tool_result(
        session,
        ctx,
        request,
        |decision| {
            let _ = reply.send(decision);
        },
    ));
    if matches!(status, Some(TurnStatus::Failed(Failure::Cancelled))) {
        commands::erase_cancelled_turn(session, ctx, events);
    }
}
