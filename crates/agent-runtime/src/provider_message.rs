//! Correlates every provider IO message with the exact in-flight request that produced it.

use agent_core::{AgentId, ContentBlock, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};
use std::collections::VecDeque;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
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

    pub(crate) fn gone(agent: AgentId, attempt: ProviderAttemptId) -> Self {
        Self {
            agent,
            attempt,
            payload: ProviderMessagePayload::Gone,
        }
    }
}

/// 只读的内省口，**仅供本 crate 的测试**：信封里的三样东西在生产代码里只由
/// [`land`] 一处消费（认领规则的唯一现场），但 117 的两条对抗测试要断言「这条
/// 消息确实回到了泵、而且它属于那个已经被划掉的 attempt」——没有内省口就只能靠
/// 「什么都没发生」来间接推断，那种测试删掉闸也照样绿。
#[cfg(test)]
impl ProviderMessage {
    pub(crate) fn agent(&self) -> &AgentId {
        &self.agent
    }

    pub(crate) fn attempt(&self) -> ProviderAttemptId {
        self.attempt
    }

    /// 载荷的种类名，够断言用，不暴露内容。
    pub(crate) fn kind(&self) -> &'static str {
        match self.payload {
            ProviderMessagePayload::Delta(_) => "delta",
            ProviderMessagePayload::Done { .. } => "done",
            ProviderMessagePayload::Gone => "gone",
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
