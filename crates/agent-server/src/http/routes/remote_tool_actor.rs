//! Remote-tool actor request/reply waits shared by HTTP routes.

use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolStatusSnapshot,
    RemoteToolSubmitDecision, RemoteToolSubmitRequest,
};

use crate::SessionHandle;
use crate::http::error::ApiError;

pub(super) async fn claim(
    handle: &SessionHandle,
    request: RemoteToolClaimRequest,
) -> Result<RemoteToolClaimDecision, ApiError> {
    let reply = handle.claim_remote_tool(request).map_err(session_dead)?;
    reply.await.map_err(|_| session_dead(()))
}

pub(super) async fn submit(
    handle: &SessionHandle,
    request: RemoteToolSubmitRequest,
) -> Result<RemoteToolSubmitDecision, ApiError> {
    let reply = handle
        .submit_remote_tool_result(request)
        .map_err(session_dead)?;
    reply.await.map_err(|_| session_dead(()))
}

pub(super) fn status(handle: &SessionHandle) -> RemoteToolStatusSnapshot {
    handle.remote_tool_status()
}

fn session_dead(_: impl Sized) -> ApiError {
    ApiError::gone("session 的 actor 在线程间请求完成前已经停止")
}
