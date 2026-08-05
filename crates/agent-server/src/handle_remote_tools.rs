//! Request/reply methods for the remote-tool actor protocol.

use tokio::sync::oneshot;

use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolStatusSnapshot,
    RemoteToolSubmitDecision, RemoteToolSubmitRequest,
};

use crate::actor::message::ActorMessage;
use crate::handle::{SessionClosed, SessionHandle};

impl SessionHandle {
    pub(crate) fn claim_remote_tool(
        &self,
        request: RemoteToolClaimRequest,
    ) -> Result<oneshot::Receiver<RemoteToolClaimDecision>, SessionClosed> {
        let (reply, response) = oneshot::channel();
        self.enqueue(ActorMessage::ClaimRemoteTool { request, reply })?;
        Ok(response)
    }

    pub(crate) fn submit_remote_tool_result(
        &self,
        request: RemoteToolSubmitRequest,
    ) -> Result<oneshot::Receiver<RemoteToolSubmitDecision>, SessionClosed> {
        let (reply, response) = oneshot::channel();
        self.enqueue(ActorMessage::SubmitRemoteToolResult { request, reply })?;
        Ok(response)
    }

    pub(crate) fn remote_tool_status(&self) -> RemoteToolStatusSnapshot {
        self.tool_status.lock().unwrap().clone()
    }
}
