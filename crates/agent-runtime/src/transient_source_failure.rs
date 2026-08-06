//! Terminal failures from a provider request that consumed transient source material.

use agent_core::{AgentId, Epoch};
use agent_transport::TransportError;

/// A terminal failure that the runtime intentionally leaves unclassified and unredacted.
///
/// The embedding host decides whether and how this data is exposed to an end user. The runtime
/// only owns the transient-source lifecycle: it purges the source material before returning this
/// value.
#[derive(Clone, Debug)]
pub enum TransientSourceFailure {
    PromptPreparation {
        agent: AgentId,
        epoch: Epoch,
    },
    InvalidCompletion {
        agent: AgentId,
        epoch: Epoch,
    },
    Transport {
        agent: AgentId,
        epoch: Epoch,
        error: TransportError,
    },
    StreamBroken {
        agent: AgentId,
        epoch: Epoch,
        message: String,
    },
    ProviderThreadGone {
        agent: AgentId,
        epoch: Epoch,
    },
    ProviderDeadlineExceeded {
        agent: AgentId,
        epoch: Epoch,
    },
}

impl TransientSourceFailure {
    pub fn agent(&self) -> &AgentId {
        match self {
            Self::PromptPreparation { agent, .. }
            | Self::InvalidCompletion { agent, .. }
            | Self::Transport { agent, .. }
            | Self::StreamBroken { agent, .. }
            | Self::ProviderThreadGone { agent, .. }
            | Self::ProviderDeadlineExceeded { agent, .. } => agent,
        }
    }

    pub fn epoch(&self) -> Epoch {
        match self {
            Self::PromptPreparation { epoch, .. }
            | Self::InvalidCompletion { epoch, .. }
            | Self::Transport { epoch, .. }
            | Self::StreamBroken { epoch, .. }
            | Self::ProviderThreadGone { epoch, .. }
            | Self::ProviderDeadlineExceeded { epoch, .. } => *epoch,
        }
    }
}
