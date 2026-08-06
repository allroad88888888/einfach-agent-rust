//! Provider 调用的收尾翻译：只消费起飞时固定下来的 [`ProviderCall`] 快照。

use std::sync::Arc;

use agent_core::{ContentBlock, Epoch, ErrorClass, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::guard;
use crate::provider_call::ProviderCall;

/// 落地：把 IO 线程的终态翻译成一个 loop 事件。
pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
    result: Result<StreamOutcome, TransportError>,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
    usage: TokenUsage,
) -> Event {
    let ProviderCall {
        agent,
        epoch,
        binding,
        guard_scope,
        drift,
        predicted_cache,
        adjustments,
        prefix,
        one_shot,
        ..
    } = call;
    if one_shot {
        return crate::transient_source_completion::finish(
            ctx,
            crate::transient_source_completion::Metadata {
                agent,
                epoch,
                guard_scope,
                drift,
                predicted_cache,
                adjustments,
                prefix,
            },
            result,
            blocks,
            stop,
        );
    }
    match result {
        Ok(StreamOutcome::Finished) => {
            guard::report_success(
                ctx,
                &agent,
                guard_scope,
                &usage,
                drift,
                predicted_cache,
                adjustments.clone(),
            );
            Event::ProviderDone {
                agent,
                epoch,
                blocks,
                stop,
                usage,
                prefix,
                adjustments,
            }
        }
        Ok(StreamOutcome::Cancelled) => Event::Cancel { agent },
        Ok(StreamOutcome::Broken(message)) => {
            transport_trouble(ctx, agent, epoch, ErrorClass::Retryable, message)
        }
        Err(TransportError::Connect { message, .. }) => {
            transport_trouble(ctx, agent, epoch, ErrorClass::Retryable, message)
        }
        Err(TransportError::Http { status, body }) => {
            let class = binding.provider.classify(status, &body);
            ctx.emit(
                &agent,
                RunnerEvent::TransportTrouble(Arc::from(format!("HTTP {status}: {body}"))),
            );
            Event::ProviderFailed {
                agent,
                epoch,
                class,
                message: Arc::from(body),
            }
        }
    }
}

/// IO 线程 panic 了（`IoMsg::Gone`）——没留下任何终态消息。
pub(crate) fn thread_gone(ctx: &mut RunnerCtx, call: ProviderCall) -> Event {
    if call.one_shot {
        ctx.transient_sources
            .purge_agent_epoch(&call.agent, call.epoch);
        return crate::transient_source_completion::provider_completion_failed(
            ctx, call.agent, call.epoch,
        );
    }
    Event::ProviderFailed {
        agent: call.agent,
        epoch: call.epoch,
        class: ErrorClass::Retryable,
        message: Arc::from("IO 线程异常退出（未留下终态消息）"),
    }
}

fn transport_trouble(
    ctx: &mut RunnerCtx,
    agent: agent_core::AgentId,
    epoch: Epoch,
    class: ErrorClass,
    message: String,
) -> Event {
    ctx.emit(
        &agent,
        RunnerEvent::TransportTrouble(Arc::from(message.as_str())),
    );
    Event::ProviderFailed {
        agent,
        epoch,
        class,
        message: Arc::from(message),
    }
}
