//! Correlates every provider IO message with the exact in-flight request that produced it.

use agent_core::{AgentId, ContentBlock, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};
use std::collections::VecDeque;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_call::{self, ProviderCall};
use crate::transient_source_failure::TransientSourceFailure;

/// One message returned by a provider IO thread.
///
/// The envelope makes `(agent, attempt)` mandatory for deltas and every terminal outcome. A
/// timed-out attempt can therefore finish after its same-agent retry starts without consuming or
/// mutating the retry's credential.
pub(crate) struct ProviderMessage {
    agent: AgentId,
    attempt: ProviderAttemptId,
    payload: ProviderMessagePayload,
}

enum ProviderMessagePayload {
    Delta(RunnerEvent),
    Done {
        result: Result<StreamOutcome, TransportError>,
        blocks: Vec<ContentBlock>,
        stop: StopReason,
        usage: TokenUsage,
    },
    PreparationFailed(ImagePreparationFailure),
    Gone,
}

impl ProviderMessage {
    pub(crate) fn delta(agent: AgentId, attempt: ProviderAttemptId, event: RunnerEvent) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::Delta(event),
        }
    }

    pub(crate) fn done(
        agent: AgentId,
        attempt: ProviderAttemptId,
        result: Result<StreamOutcome, TransportError>,
        blocks: Vec<ContentBlock>,
        stop: StopReason,
        usage: TokenUsage,
    ) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::Done {
                result,
                blocks,
                stop,
                usage,
            },
        }
    }

    pub(crate) fn preparation_failed(
        agent: AgentId,
        attempt: ProviderAttemptId,
        failure: ImagePreparationFailure,
    ) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::PreparationFailed(failure),
        }
    }

    pub(crate) fn gone(agent: AgentId, attempt: ProviderAttemptId) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::Gone,
        }
    }
}

/// Land a provider message only when its exact launch credential is still in flight.
pub(crate) fn land(
    ctx: &mut RunnerCtx,
    calls: &mut Vec<ProviderCall>,
    pending: &mut VecDeque<Event>,
    message: ProviderMessage,
) -> Option<TransientSourceFailure> {
    let ProviderMessage {
        agent,
        attempt,
        payload,
    } = message;
    let Some(at) = calls
        .iter()
        .position(|call| call.agent == agent && call.attempt == attempt)
    else {
        return None;
    };

    match payload {
        ProviderMessagePayload::Delta(event) => {
            let call = &mut calls[at];
            let agent = call.agent.clone();
            if let Some(event) = provider_call::gate_delta(call, event) {
                ctx.emit(&agent, event);
            }
            None
        }
        ProviderMessagePayload::Done {
            result,
            blocks,
            stop,
            usage,
        } => {
            let call = calls.remove(at);
            match provider_call::finish(ctx, call, result, blocks, stop, usage) {
                Ok(event) => {
                    pending.push_back(event);
                    None
                }
                Err(failure) => Some(failure),
            }
        }
        ProviderMessagePayload::PreparationFailed(failure) => {
            let call = calls.remove(at);
            pending.push_back(provider_call::preparation_failed(ctx, call, failure));
            None
        }
        ProviderMessagePayload::Gone => {
            let call = calls.remove(at);
            match provider_call::thread_gone(ctx, call) {
                Ok(event) => {
                    pending.push_back(event);
                    None
                }
                Err(failure) => Some(failure),
            }
        }
    }
}
