//! `srv:vision/inspect` 的 runtime 门面与隔离 child 启动。
//!
//! 模型只提交图片句柄和自包含问题；provider、model、endpoint、key 与 timeout 全由
//! 服务端注入的 `vision` execution binding 固定。这个门面只给 root，child 一律
//! 看不见；视觉 child 自身的授权工具集恒为空。

use std::sync::Arc;

use agent_core::vision::{VisionFailure, VisionToolOutcome, parse_vision_inspect_request};
use agent_core::{
    AgentId, ChildConfig, Epoch, Event, ExecutionProfileId, Session, ToolCallId, UserImage,
};
use serde_json::Value;

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::subtree::Subtree;
use crate::{persist, reply};

pub(crate) use agent_core::vision::VISION_INSPECT_TOOL;

const VISION_PROFILE: &str = "vision";
const PROVIDER_NEUTRAL_IMAGE_MIME: &str = "application/octet-stream";

pub(crate) fn is_enabled(ctx: &RunnerCtx) -> bool {
    ctx.execution_bindings.contains_key(&profile_id())
}

pub(crate) fn is_root(session: &Session, agent: &AgentId) -> bool {
    agent == session.agent()
}

pub(crate) fn intercept(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    parent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    let request = ctx.tools.snapshot(VISION_INSPECT_TOOL, Arc::clone(input));
    ctx.emit(
        parent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    if !is_enabled(ctx) {
        return finish(
            ctx,
            parent,
            call_id,
            epoch,
            VisionToolOutcome::failure(VisionFailure::profile_unavailable()),
        );
    }
    let request = match parse_vision_inspect_request(input) {
        Ok(request) => request,
        Err(failure) => {
            return finish(
                ctx,
                parent,
                call_id,
                epoch,
                VisionToolOutcome::failure(failure),
            );
        }
    };

    let child = match session.spawn_child(
        parent,
        ChildConfig {
            tools_allowed: Vec::new(),
            execution_profile: Some(profile_id()),
            max_retries: Some(0),
        },
    ) {
        Ok(child) => child,
        Err(_) => {
            return finish(
                ctx,
                parent,
                call_id,
                epoch,
                VisionToolOutcome::failure(VisionFailure::child_failed(false)),
            );
        }
    };
    persist::sync(ctx, session);
    subtree.record_vision(
        child.clone(),
        parent.clone(),
        call_id,
        epoch,
        request.images().len(),
    );

    let images = request
        .images()
        .iter()
        .map(|handle| UserImage {
            reference: handle.attachment_reference(),
            // Vault materialization replaces the provider-neutral reference before encoding.
            mime: Arc::from(PROVIDER_NEUTRAL_IMAGE_MIME),
            name: None,
        })
        .collect();
    Dispatched::Event(Event::UserInput {
        agent: child,
        text: Arc::from(request.question()),
        images,
    })
}

/// reserved facade 被 child 猜中时固定拒绝。它不经过 ToolTable，因此宿主伪造的
/// 同名本地/远端声明也不能改变执行位置或拿到输入。
pub(crate) fn refuse_non_root(
    ctx: &mut RunnerCtx,
    child: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
) -> Dispatched {
    reply::refuse(
        ctx,
        child,
        call_id,
        epoch,
        VISION_INSPECT_TOOL,
        "[unknown_tool] unknown tool".to_string(),
    )
}

fn profile_id() -> ExecutionProfileId {
    ExecutionProfileId::new(VISION_PROFILE)
}

fn finish(
    ctx: &mut RunnerCtx,
    parent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    outcome: VisionToolOutcome,
) -> Dispatched {
    reply::settle(
        ctx,
        parent,
        call_id,
        epoch,
        VISION_INSPECT_TOOL,
        outcome.content.to_string(),
        outcome.is_error,
    )
}

#[cfg(test)]
#[path = "vision_tool_tests.rs"]
mod tests;
