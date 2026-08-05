//! One-shot prompt overlay for transient source calls.

use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Epoch, Message, Role};

use crate::transient_source_recovery::current_source_result_ids;
use crate::transient_source_vault::{
    TransientSourceReasoning, TransientSourceSecret, TransientSourceVault,
};

pub(crate) struct PreparedPrompt {
    pub(crate) messages: Vec<Message>,
    pub(crate) one_shot: bool,
}

pub(crate) fn prepare(
    messages: &[Message],
    vault: &mut TransientSourceVault,
    agent: &AgentId,
    epoch: Epoch,
) -> Result<PreparedPrompt, ()> {
    let expected = current_source_result_ids(messages);
    let Some(replay) = vault.take_ready_hop(agent, epoch, &expected)? else {
        if !expected.is_empty() {
            return Err(());
        }
        return Ok(PreparedPrompt {
            messages: messages.to_vec(),
            one_shot: false,
        });
    };
    let mut overlaid = messages.to_vec();
    overlay_current(&mut overlaid, replay.current)?;
    overlay_reasoning(&mut overlaid, &replay.reasoning)?;
    Ok(PreparedPrompt {
        messages: overlaid,
        one_shot: true,
    })
}

fn overlay_current(
    messages: &mut [Message],
    secrets: Vec<TransientSourceSecret>,
) -> Result<(), ()> {
    let mut source_message = None;
    for secret in secrets {
        let mut use_count = 0;
        let mut result_count = 0;
        let mut use_message = None;
        for (message_index, message) in messages.iter_mut().enumerate() {
            for block in &mut message.blocks {
                match block {
                    ContentBlock::ToolUse { id, name, input }
                        if *id == secret.call_id && *name == secret.tool =>
                    {
                        *input = Arc::clone(&secret.input);
                        use_count += 1;
                        use_message = Some(message_index);
                    }
                    ContentBlock::ToolResult {
                        id,
                        content,
                        is_error,
                    } if *id == secret.call_id => {
                        *content = Arc::clone(&secret.outcome);
                        *is_error = secret.is_error;
                        result_count += 1;
                    }
                    _ => {}
                }
            }
        }
        if use_count != 1 || result_count != 1 {
            return Err(());
        }
        if source_message.is_some() && source_message != use_message {
            return Err(());
        }
        source_message = use_message;
    }
    Ok(())
}

fn overlay_reasoning(
    messages: &mut [Message],
    records: &[TransientSourceReasoning],
) -> Result<(), ()> {
    let mut seen = vec![false; records.len()];
    for message in messages {
        let matching: Vec<usize> = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { id, name, .. } => records
                    .iter()
                    .position(|record| record.call_id == *id && record.tool == *name),
                _ => None,
            })
            .collect();
        if matching.is_empty() {
            continue;
        }
        let reasoning = records[matching[0]].reasoning.clone();
        if message.role != Role::Assistant
            || message
                .blocks
                .iter()
                .any(|block| matches!(block, ContentBlock::Thinking(_)))
            || matching
                .iter()
                .any(|index| records[*index].reasoning.as_deref() != reasoning.as_deref())
            || matching.iter().any(|index| seen[*index])
            || matching
                .iter()
                .enumerate()
                .any(|(at, index)| matching[..at].contains(index))
        {
            return Err(());
        }
        for index in matching {
            seen[index] = true;
        }
        if let Some(reasoning) = reasoning {
            message.blocks.insert(0, ContentBlock::Thinking(reasoning));
        }
    }
    if seen.iter().any(|seen| !seen) {
        return Err(());
    }
    Ok(())
}
