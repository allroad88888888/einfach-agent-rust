//! Wire representation of an unmodified terminal transient-source provider failure.

use agent_runtime::TransientSourceFailure as RuntimeTransientSourceFailure;
use agent_transport::TransportError;
use serde::{Deserialize, Serialize};

/// The original terminal failure from a provider request that consumed transient source data.
///
/// This is a wire adapter, not an error policy: provider messages and HTTP response bodies are
/// transferred without redaction or classification. The outer host owns any presentation policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct TransientSourceFailureEvent {
    pub epoch: u64,
    pub cause: TransientSourceFailureCause,
}

/// The raw terminal cause, made serializable for the server protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum TransientSourceFailureCause {
    PromptPreparation,
    InvalidCompletion,
    TransportConnect { attempts: u32, message: String },
    TransportHttp { status: u16, body: String },
    StreamBroken { message: String },
    ProviderThreadGone,
    ProviderDeadlineExceeded,
}

impl From<RuntimeTransientSourceFailure> for TransientSourceFailureEvent {
    fn from(failure: RuntimeTransientSourceFailure) -> Self {
        match failure {
            RuntimeTransientSourceFailure::PromptPreparation { epoch, .. } => Self {
                epoch: epoch.0,
                cause: TransientSourceFailureCause::PromptPreparation,
            },
            RuntimeTransientSourceFailure::InvalidCompletion { epoch, .. } => Self {
                epoch: epoch.0,
                cause: TransientSourceFailureCause::InvalidCompletion,
            },
            RuntimeTransientSourceFailure::Transport { epoch, error, .. } => {
                let cause = match error {
                    TransportError::Connect { attempts, message } => {
                        TransientSourceFailureCause::TransportConnect { attempts, message }
                    }
                    TransportError::Http { status, body } => {
                        TransientSourceFailureCause::TransportHttp { status, body }
                    }
                };
                Self {
                    epoch: epoch.0,
                    cause,
                }
            }
            RuntimeTransientSourceFailure::StreamBroken { epoch, message, .. } => Self {
                epoch: epoch.0,
                cause: TransientSourceFailureCause::StreamBroken { message },
            },
            RuntimeTransientSourceFailure::ProviderThreadGone { epoch, .. } => Self {
                epoch: epoch.0,
                cause: TransientSourceFailureCause::ProviderThreadGone,
            },
            RuntimeTransientSourceFailure::ProviderDeadlineExceeded { epoch, .. } => Self {
                epoch: epoch.0,
                cause: TransientSourceFailureCause::ProviderDeadlineExceeded,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use agent_core::{AgentId, Epoch};

    use super::*;

    #[test]
    fn transport_http_keeps_the_original_body() {
        let event = TransientSourceFailureEvent::from(RuntimeTransientSourceFailure::Transport {
            agent: AgentId::root(),
            epoch: Epoch(4),
            error: TransportError::Http {
                status: 502,
                body: "upstream diagnostic".to_string(),
            },
        });

        assert_eq!(
            event,
            TransientSourceFailureEvent {
                epoch: 4,
                cause: TransientSourceFailureCause::TransportHttp {
                    status: 502,
                    body: "upstream diagnostic".to_string(),
                },
            }
        );
    }

    #[test]
    fn wire_round_trip_keeps_the_original_stream_message() {
        let event = TransientSourceFailureEvent {
            epoch: 9,
            cause: TransientSourceFailureCause::StreamBroken {
                message: "provider diagnostic with request detail".to_string(),
            },
        };

        let encoded = serde_json::to_string(&event).expect("serialize raw failure event");
        assert!(encoded.contains("provider diagnostic with request detail"));
        assert_eq!(
            serde_json::from_str::<TransientSourceFailureEvent>(&encoded)
                .expect("deserialize raw failure event"),
            event
        );
    }
}
