//! Provider 调用的收尾翻译：只消费起飞时固定下来的 [`ProviderCall`] 快照。

use std::sync::Arc;

use agent_core::{ContentBlock, Epoch, ErrorClass, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::guard;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::provider_call::ProviderCall;
use crate::transient_source_failure::TransientSourceFailure;

/// 落地：把 IO 线程的终态翻译成一个 loop 事件。
pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
    result: Result<StreamOutcome, TransportError>,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
    usage: TokenUsage,
) -> Result<Event, TransientSourceFailure> {
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
        replay_terminal_deltas,
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
            if replay_terminal_deltas {
                replay_deltas(ctx, &agent, &blocks);
            }
            guard::report_success(
                ctx,
                &agent,
                guard_scope,
                &usage,
                drift,
                predicted_cache,
                adjustments.clone(),
            );
            Ok(Event::ProviderDone {
                agent,
                epoch,
                blocks,
                stop,
                usage,
                prefix,
                adjustments,
            })
        }
        Ok(StreamOutcome::Cancelled) => Ok(Event::Cancel { agent }),
        Ok(StreamOutcome::Broken(message)) => Ok(transport_trouble(
            ctx,
            agent,
            epoch,
            ErrorClass::Retryable,
            message,
        )),
        Err(TransportError::Connect { message, .. }) => Ok(transport_trouble(
            ctx,
            agent,
            epoch,
            ErrorClass::Retryable,
            message,
        )),
        Err(TransportError::Http { status, body }) => {
            let class = binding.provider.classify(status, &body);
            ctx.emit(
                &agent,
                RunnerEvent::TransportTrouble(Arc::from(format!("HTTP {status}: {body}"))),
            );
            Ok(Event::ProviderFailed {
                agent,
                epoch,
                class,
                message: Arc::from(body),
            })
        }
    }
}

fn replay_deltas(ctx: &mut RunnerCtx, agent: &agent_core::AgentId, blocks: &[ContentBlock]) {
    for block in blocks {
        let event = match block {
            ContentBlock::Text(text) => RunnerEvent::TextDelta(Arc::clone(text)),
            ContentBlock::Thinking(text) => RunnerEvent::ThinkingDelta(Arc::clone(text)),
            ContentBlock::ToolUse { name, .. } => RunnerEvent::ToolCallStarted {
                name: Arc::clone(name),
            },
            ContentBlock::ToolResult { .. } | ContentBlock::Image { .. } => continue,
        };
        ctx.emit(agent, event);
    }
}

/// IO 线程 panic 了（provider message 的 `Gone` 终态）——没留下任何终态消息。
pub(crate) fn thread_gone(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
) -> Result<Event, TransientSourceFailure> {
    if call.one_shot {
        ctx.transient_sources
            .purge_agent_epoch(&call.agent, call.epoch);
        return Err(TransientSourceFailure::ProviderThreadGone {
            agent: call.agent,
            epoch: call.epoch,
        });
    }
    Ok(Event::ProviderFailed {
        agent: call.agent,
        epoch: call.epoch,
        class: ErrorClass::Retryable,
        message: Arc::from("IO 线程异常退出（未留下终态消息）"),
    })
}

pub(crate) fn preparation_failed(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
    failure: ImagePreparationFailure,
) -> Event {
    preparation_start_failed(ctx, call.agent, call.epoch, call.one_shot, failure)
}

pub(crate) fn preparation_start_failed(
    ctx: &mut RunnerCtx,
    agent: agent_core::AgentId,
    epoch: Epoch,
    one_shot: bool,
    failure: ImagePreparationFailure,
) -> Event {
    if one_shot {
        ctx.transient_sources.purge_agent_epoch(&agent, epoch);
    }
    ctx.record_image_preparation_failure(agent.clone(), failure);
    if failure == ImagePreparationFailure::Cancelled {
        return Event::Cancel { agent };
    }
    Event::ProviderFailed {
        agent,
        epoch,
        class: failure.error_class(),
        message: Arc::from(failure.message()),
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
