//! Whole-batch scrub before an external provider event reaches `Session::step`.

use std::sync::Arc;

use agent_core::{ContentBlock, ErrorClass, Event, Session, StopReason, ToolCallId};

use crate::ctx::RunnerCtx;
use crate::transient_source_policy::{SAFE_INGRESS_ERROR, is_transient_source, placeholder_input};
use crate::transient_source_vault::CapturedSource;

pub(crate) fn prepare(session: &Session, ctx: &mut RunnerCtx, event: Event) -> Event {
    let Event::ProviderDone {
        agent,
        epoch,
        blocks,
        stop,
        usage,
        prefix,
        adjustments,
    } = event
    else {
        return event;
    };
    if !blocks.iter().any(is_source_block) {
        return Event::ProviderDone {
            agent,
            epoch,
            blocks,
            stop,
            usage,
            prefix,
            adjustments,
        };
    }

    let active = session.active_skills_of(&agent);
    let reasoning = reasoning_content(&blocks);
    let mut seen = Vec::<ToolCallId>::new();
    let mut captured = Vec::new();
    let mut sanitized = Vec::with_capacity(blocks.len());
    let mut valid = matches!(stop, StopReason::ToolUse);
    for block in blocks {
        match block {
            ContentBlock::ToolUse { id, name, input } => {
                if seen.contains(&id) {
                    valid = false;
                }
                seen.push(id.clone());
                if is_transient_source(&name) {
                    let declared = ctx.tools.declares(&name)
                        || ctx
                            .tools
                            .active_host_tool_request(&active, &name, Arc::clone(&input))
                            .is_some();
                    valid &= declared;
                    captured.push(CapturedSource {
                        agent: agent.clone(),
                        epoch,
                        call_id: id.clone(),
                        tool: Arc::clone(&name),
                        input,
                        reasoning: reasoning.clone(),
                    });
                    sanitized.push(ContentBlock::ToolUse {
                        id,
                        name,
                        input: placeholder_input(),
                    });
                } else {
                    sanitized.push(ContentBlock::ToolUse { id, name, input });
                }
            }
            // A source-producing generation is sensitive as a whole. Text and thinking
            // siblings can echo the just-generated arguments, so they never cross the
            // durable/event boundary. Provider responses cannot legitimately contain
            // tool results alongside a tool request; fail the whole batch closed.
            ContentBlock::Text(_) | ContentBlock::Thinking(_) => {}
            ContentBlock::ToolResult { .. } => valid = false,
        }
    }
    if !valid || ctx.transient_sources.capture_batch(captured).is_err() {
        ctx.transient_sources.purge_agent_epoch(&agent, epoch);
        return Event::ProviderFailed {
            agent,
            epoch,
            class: ErrorClass::BadRequest,
            message: Arc::from(SAFE_INGRESS_ERROR),
        };
    }
    Event::ProviderDone {
        agent,
        epoch,
        blocks: sanitized,
        stop,
        usage,
        prefix,
        adjustments,
    }
}

fn is_source_block(block: &ContentBlock) -> bool {
    matches!(block, ContentBlock::ToolUse { name, .. } if is_transient_source(name))
}

/// Preserve the provider's bytes exactly; multiple stream fragments have already been
/// coalesced into ordered `Thinking` blocks by the provider accumulator.
fn reasoning_content(blocks: &[ContentBlock]) -> Option<Arc<str>> {
    let mut value = String::new();
    let mut found = false;
    for block in blocks {
        if let ContentBlock::Thinking(text) = block {
            found = true;
            value.push_str(text);
        }
    }
    found.then(|| Arc::from(value))
}
