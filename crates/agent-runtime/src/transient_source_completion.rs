//! Final gate for provider calls that consumed process-local transient source material.

use std::sync::Arc;

use agent_core::{
    Adjustment, AgentId, ContentBlock, DriftVerdict, Epoch, ErrorClass, Event, PrefixImage,
    StopReason, TokenUsage,
};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::guard;
use crate::transient_source_policy::{SAFE_CANDIDATE, SAFE_PROVIDER_ERROR, is_transient_source};

const PROVIDER_COMPLETION_FAILED: &str = "provider_completion_failed";

enum PrivateCompletion {
    Terminal {
        candidate: Vec<Arc<str>>,
        stop: StopReason,
    },
    SourceTools(Vec<ContentBlock>),
}

pub(crate) struct Metadata {
    pub(crate) agent: AgentId,
    pub(crate) epoch: Epoch,
    pub(crate) drift: DriftVerdict,
    pub(crate) predicted_cache: u32,
    pub(crate) adjustments: Vec<Adjustment>,
    pub(crate) prefix: PrefixImage,
}

pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    metadata: Metadata,
    result: Result<StreamOutcome, TransportError>,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
) -> Event {
    match result {
        Ok(StreamOutcome::Finished) => match private_completion(blocks, stop) {
            Ok(PrivateCompletion::Terminal { candidate, stop }) => {
                ctx.transient_sources
                    .purge_agent_epoch(&metadata.agent, metadata.epoch);
                emit_terminal_candidate(ctx, &metadata.agent, candidate);
                success(
                    ctx,
                    metadata,
                    vec![ContentBlock::Text(Arc::from(SAFE_CANDIDATE))],
                    stop,
                )
            }
            Ok(PrivateCompletion::SourceTools(blocks)) => {
                success(ctx, metadata, blocks, StopReason::ToolUse)
            }
            Err(()) => {
                ctx.transient_sources
                    .purge_agent_epoch(&metadata.agent, metadata.epoch);
                provider_completion_failed(ctx, metadata.agent, metadata.epoch)
            }
        },
        Ok(StreamOutcome::Cancelled) => {
            ctx.transient_sources
                .purge_agent_epoch(&metadata.agent, metadata.epoch);
            Event::Cancel {
                agent: metadata.agent,
            }
        }
        Ok(StreamOutcome::Broken(_)) | Err(_) => {
            ctx.transient_sources
                .purge_agent_epoch(&metadata.agent, metadata.epoch);
            provider_completion_failed(ctx, metadata.agent, metadata.epoch)
        }
    }
}

pub(crate) fn provider_completion_failed(
    ctx: &mut RunnerCtx,
    agent: AgentId,
    epoch: Epoch,
) -> Event {
    observed_failure(ctx, agent, epoch, PROVIDER_COMPLETION_FAILED)
}

fn observed_failure(
    ctx: &mut RunnerCtx,
    agent: AgentId,
    epoch: Epoch,
    reason: &'static str,
) -> Event {
    ctx.emit(&agent, RunnerEvent::TransportTrouble(Arc::from(reason)));
    failure(agent, epoch)
}

pub(crate) fn failure(agent: AgentId, epoch: Epoch) -> Event {
    Event::ProviderFailed {
        agent,
        epoch,
        class: ErrorClass::Unknown,
        message: Arc::from(SAFE_PROVIDER_ERROR),
    }
}

fn private_completion(
    blocks: Vec<ContentBlock>,
    stop: StopReason,
) -> Result<PrivateCompletion, ()> {
    match stop {
        StopReason::EndTurn | StopReason::MaxTokens | StopReason::StopSequence => {
            let candidate = terminal_text(blocks)?;
            Ok(PrivateCompletion::Terminal { candidate, stop })
        }
        StopReason::ToolUse => source_tool_uses(blocks).map(PrivateCompletion::SourceTools),
        StopReason::Other(_) => Err(()),
    }
}

fn terminal_text(blocks: Vec<ContentBlock>) -> Result<Vec<Arc<str>>, ()> {
    let mut candidate = Vec::new();
    for block in blocks {
        match block {
            ContentBlock::Text(text) => candidate.push(text),
            ContentBlock::Thinking(_) => {}
            ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Image { .. } => return Err(()),
        }
    }
    Ok(candidate)
}

fn emit_terminal_candidate(ctx: &mut RunnerCtx, agent: &AgentId, candidate: Vec<Arc<str>>) {
    for text in candidate {
        ctx.emit(agent, RunnerEvent::TextDelta(text));
    }
}

fn source_tool_uses(blocks: Vec<ContentBlock>) -> Result<Vec<ContentBlock>, ()> {
    let mut private_blocks = Vec::new();
    let mut source_count = 0;
    for block in blocks {
        match block {
            // This event is handed straight to transient_source_ingress. Keeping Thinking
            // here lets the next private hop replay it; ingress removes it before step,
            // persistence, or public emission can observe the event.
            ContentBlock::Thinking(_) => private_blocks.push(block),
            ContentBlock::ToolUse { ref name, .. } if is_transient_source(name) => {
                source_count += 1;
                private_blocks.push(block);
            }
            ContentBlock::Text(_)
            | ContentBlock::ToolUse { .. }
            | ContentBlock::ToolResult { .. }
            | ContentBlock::Image { .. } => return Err(()),
        }
    }
    if source_count == 0 {
        return Err(());
    }
    Ok(private_blocks)
}

fn success(
    ctx: &mut RunnerCtx,
    metadata: Metadata,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
) -> Event {
    let usage = TokenUsage {
        prompt: 0,
        completion: 0,
        cached: None,
    };
    guard::report_success(
        ctx,
        &metadata.agent,
        &usage,
        metadata.drift,
        metadata.predicted_cache,
        metadata.adjustments.clone(),
    );
    Event::ProviderDone {
        agent: metadata.agent,
        epoch: metadata.epoch,
        blocks,
        stop,
        usage,
        prefix: metadata.prefix,
        adjustments: metadata.adjustments,
    }
}
