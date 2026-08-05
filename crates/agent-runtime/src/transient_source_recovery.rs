//! Recovery marker detection for process-local transient source state.

use agent_core::{ContentBlock, Message, Session, ToolCallId, TurnStatus};

use crate::transient_source_policy::{
    SAFE_ERROR, SAFE_RESULT, is_placeholder_input, is_transient_source,
};

/// A recovered source call cannot resume because its raw input/outcome vault was process-local.
pub fn recovered_transient_source_needs_fail_close(session: &Session) -> bool {
    session.live_agents().into_iter().any(|agent| {
        let messages: Vec<Message> = session.messages_of(&agent).iter().cloned().collect();
        match session.status_of(&agent) {
            TurnStatus::ToolsPending => latest_has_source_use(&messages),
            TurnStatus::Thinking => !current_source_result_ids(&messages).is_empty(),
            _ => false,
        }
    })
}

pub(crate) fn current_source_result_ids(messages: &[Message]) -> Vec<ToolCallId> {
    let Some(last) = messages.last() else {
        return Vec::new();
    };
    last.blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { id, content, .. }
                if matches!(&**content, SAFE_RESULT | SAFE_ERROR)
                    && has_source_use(messages, id) =>
            {
                Some(id.clone())
            }
            _ => None,
        })
        .collect()
}

fn latest_has_source_use(messages: &[Message]) -> bool {
    messages.last().is_some_and(|message| {
        message.blocks.iter().any(|block| match block {
            ContentBlock::ToolUse { name, input, .. } => {
                is_transient_source(name) && is_placeholder_input(input)
            }
            _ => false,
        })
    })
}

fn has_source_use(messages: &[Message], target: &ToolCallId) -> bool {
    messages.iter().any(|message| {
        message.blocks.iter().any(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                id == target && is_transient_source(name) && is_placeholder_input(input)
            }
            _ => false,
        })
    })
}
