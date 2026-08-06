//! 前台子 agent 等待槽及其终态回写。
//!
//! 这里只回答一件事：哪个 child 占着父的哪个 tool-call 槽，以及 child 终止后该
//! 如何把结果写回该槽。后台 child 的 detached/stash 生命周期仍由 `subtree` 管。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, Session, ToolCallId};

use crate::child_outcome;
use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

struct ChildSlot {
    child: AgentId,
    parent: AgentId,
    call_id: ToolCallId,
    /// 发起 tool call 时的世代；迟到结果必须继续经过 core 的 epoch 闸。
    epoch: Epoch,
    tool: &'static str,
    outcome: ChildSlotOutcome,
}

enum ChildSlotOutcome {
    Generic,
    Vision {
        images_inspected: usize,
        timed_out: bool,
    },
}

pub(crate) struct SlotWriteback {
    pub(crate) child: AgentId,
    pub(crate) event: Event,
}

#[derive(Default)]
pub(crate) struct ChildSlots {
    slots: Vec<ChildSlot>,
}

impl ChildSlots {
    pub(crate) fn record(
        &mut self,
        child: AgentId,
        parent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        tool: &'static str,
    ) {
        self.slots.push(ChildSlot {
            child,
            parent,
            call_id,
            epoch,
            tool,
            outcome: ChildSlotOutcome::Generic,
        });
    }

    pub(crate) fn record_vision(
        &mut self,
        child: AgentId,
        parent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        images_inspected: usize,
    ) {
        self.slots.push(ChildSlot {
            child,
            parent,
            call_id,
            epoch,
            tool: crate::vision_tool::VISION_INSPECT_TOOL,
            outcome: ChildSlotOutcome::Vision {
                images_inspected,
                timed_out: false,
            },
        });
    }

    /// Core 会把 timeout 和其它 retryable provider 失败归为同一类；这里只保留
    /// runtime deadline 的来源位，用于生成稳定 `vision_timeout` 信封。
    pub(crate) fn record_provider_timeout(&mut self, child: &AgentId) {
        let Some(slot) = self.slots.iter_mut().find(|slot| &slot.child == child) else {
            return;
        };
        if let ChildSlotOutcome::Vision { timed_out, .. } = &mut slot.outcome {
            *timed_out = true;
        }
    }

    pub(crate) fn record_provider_start(&mut self, child: &AgentId) {
        let Some(slot) = self.slots.iter_mut().find(|slot| &slot.child == child) else {
            return;
        };
        if let ChildSlotOutcome::Vision { timed_out, .. } = &mut slot.outcome {
            *timed_out = false;
        }
    }

    pub(crate) fn contains(&self, child: &AgentId) -> bool {
        self.slots.iter().any(|slot| &slot.child == child)
    }

    /// 终态 child 按槽位登记顺序回写；未终止的槽保持原位。
    pub(crate) fn harvest(&mut self, session: &Session, ctx: &mut RunnerCtx) -> Vec<SlotWriteback> {
        let mut writebacks = Vec::new();
        let mut i = 0;
        while i < self.slots.len() {
            let status = session.status_of(&self.slots[i].child);
            if !status.is_terminal() {
                i += 1;
                continue;
            }
            let slot = self.slots.remove(i);
            let (content, is_error) = match slot.outcome {
                ChildSlotOutcome::Generic => child_outcome::outcome(session, &slot.child, &status),
                ChildSlotOutcome::Vision {
                    images_inspected,
                    timed_out,
                } => {
                    let preparation_failure = ctx.take_image_preparation_failure(&slot.child);
                    crate::vision_child_outcome::outcome(
                        session,
                        &slot.child,
                        &status,
                        images_inspected,
                        timed_out,
                        preparation_failure,
                    )
                }
            };
            ctx.emit(
                &slot.parent,
                RunnerEvent::ToolExecuted {
                    call_id: slot.call_id.clone(),
                    tool: Arc::from(slot.tool),
                    output_len: content.len(),
                    is_error,
                },
            );
            let event = if is_error {
                Event::ToolFailed {
                    agent: slot.parent,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    error: Arc::from(content),
                }
            } else {
                Event::ToolResult {
                    agent: slot.parent,
                    epoch: slot.epoch,
                    call_id: slot.call_id,
                    content: Arc::from(content),
                }
            };
            writebacks.push(SlotWriteback {
                child: slot.child,
                event,
            });
        }
        writebacks
    }
}
