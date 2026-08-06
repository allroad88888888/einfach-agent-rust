//! Correlates every provider IO message with the exact in-flight request that produced it.

use std::collections::VecDeque;
use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_call::{self, ProviderCall};

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
        private_references: Vec<Arc<str>>,
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
        private_references: Vec<Arc<str>>,
    ) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::Done {
                result,
                blocks,
                stop,
                usage,
                private_references,
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
) {
    let ProviderMessage {
        agent,
        attempt,
        payload,
    } = message;
    let Some(at) = calls
        .iter()
        .position(|call| call.agent == agent && call.attempt == attempt)
    else {
        return;
    };

    match payload {
        ProviderMessagePayload::Delta(event) => {
            let call = &mut calls[at];
            let agent = call.agent.clone();
            if let Some(event) = provider_call::gate_delta(call, event) {
                ctx.emit(&agent, event);
            }
        }
        ProviderMessagePayload::Done {
            result,
            blocks,
            stop,
            usage,
            private_references,
        } => {
            let call = calls.remove(at);
            pending.push_back(provider_call::finish(
                ctx,
                call,
                result,
                blocks,
                stop,
                usage,
                private_references,
            ));
        }
        ProviderMessagePayload::PreparationFailed(failure) => {
            let call = calls.remove(at);
            pending.push_back(provider_call::preparation_failed(ctx, call, failure));
        }
        ProviderMessagePayload::Gone => {
            let call = calls.remove(at);
            pending.push_back(provider_call::thread_gone(ctx, call));
        }
    }
}
