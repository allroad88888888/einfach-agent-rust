//! Internal actor inbox envelope, including non-serializable one-shot replies.

use tokio::sync::oneshot;

use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitRequest,
};

use crate::command::Command;

/// Values sent to the session actor. Only [`Command`] is part of the serializable public
/// protocol; remote-tool request/reply messages deliberately keep their one-shot senders here.
pub(crate) enum ActorMessage {
    Command(Command),
    ClaimRemoteTool {
        request: RemoteToolClaimRequest,
        reply: oneshot::Sender<RemoteToolClaimDecision>,
    },
    SubmitRemoteToolResult {
        request: RemoteToolSubmitRequest,
        reply: oneshot::Sender<RemoteToolSubmitDecision>,
    },
}
