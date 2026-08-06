//! Provider 调用的收尾翻译：只消费起飞时固定下来的 [`ProviderCall`] 快照。

use std::sync::Arc;

use agent_core::{ContentBlock, Epoch, ErrorClass, Event, StopReason, TokenUsage};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::guard;
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::provider_call::ProviderCall;

/// 落地：把 IO 线程的终态翻译成一个 loop 事件。
pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
    result: Result<StreamOutcome, TransportError>,
    mut blocks: Vec<ContentBlock>,
    mut stop: StopReason,
    usage: TokenUsage,
    private_references: Vec<Arc<str>>,
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
        replay_sanitized_deltas,
        redact_provider_errors,
        ..
    } = call;
    crate::vision_output_privacy::scrub_terminal(&mut blocks, &mut stop, &private_references);
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
            if replay_sanitized_deltas {
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
        Ok(StreamOutcome::Broken(message)) => transport_trouble(
            ctx,
            agent,
            epoch,
            ErrorClass::Retryable,
            scrub_provider_text(message, &private_references),
            redact_provider_errors,
        ),
        Err(TransportError::Connect { message, .. }) => transport_trouble(
            ctx,
            agent,
            epoch,
            ErrorClass::Retryable,
            scrub_provider_text(message, &private_references),
            redact_provider_errors,
        ),
        Err(TransportError::Http { status, body }) => {
            let class = binding.provider.classify(status, &body);
            if redact_provider_errors {
                return private_provider_failure(ctx, agent, epoch, class);
            }
            let body = scrub_provider_text(body, &private_references);
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

fn scrub_provider_text(message: String, private_references: &[Arc<str>]) -> String {
    crate::vision_output_privacy::scrub_text(&message, private_references)
}

/// IO 线程 panic 了（provider message 的 `Gone` 终态）——没留下任何终态消息。
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
    redact_provider_errors: bool,
) -> Event {
    if redact_provider_errors {
        return private_provider_failure(ctx, agent, epoch, class);
    }
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

fn private_provider_failure(
    ctx: &mut RunnerCtx,
    agent: agent_core::AgentId,
    epoch: Epoch,
    class: ErrorClass,
) -> Event {
    const MESSAGE: &str = "private execution profile provider request failed";
    ctx.emit(&agent, RunnerEvent::TransportTrouble(Arc::from(MESSAGE)));
    Event::ProviderFailed {
        agent,
        epoch,
        class,
        message: Arc::from(MESSAGE),
    }
}
