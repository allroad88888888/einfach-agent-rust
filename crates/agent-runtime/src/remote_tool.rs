//! Web 宿主完成工具调用后的受控回传。
//!
//! `RunnerCtx` 只接受已经由 dispatch 登记的精确 `(agent, call_id)`；客户端不能传
//! epoch，也不能把结果塞进任意本地工具槽。验证通过后再恢复同一条事件泵。

use std::fmt;
use std::sync::Arc;

use agent_core::{AgentId, Event, Session, ToolCallId, TurnStatus};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::runner;

/// 远端宿主确认工具执行的两种结果。
pub enum RemoteToolOutput {
    Success(String),
    Failure(String),
}

/// 回传不对应当前等待槽位时的安全拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteToolResultError {
    agent: AgentId,
    call_id: ToolCallId,
}

impl fmt::Display for RemoteToolResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "远端工具回传不匹配当前等待调用（agent={}, call_id={}）",
            self.agent.as_str(), self.call_id.0
        )
    }
}

impl std::error::Error for RemoteToolResultError {}

/// 校验并消费一个等待中的远端工具调用，然后从该结果恢复事件泵。
pub fn resolve_remote_tool(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: AgentId,
    call_id: ToolCallId,
    output: RemoteToolOutput,
) -> Result<TurnStatus, RemoteToolResultError> {
    let pending = ctx.take_remote_tool(&agent, &call_id).ok_or_else(|| RemoteToolResultError {
        agent: agent.clone(),
        call_id: call_id.clone(),
    })?;
    let (event, output_len, is_error) = match output {
        RemoteToolOutput::Success(content) => (
            Event::ToolResult {
                agent: pending.agent.clone(),
                epoch: pending.epoch,
                call_id: pending.call_id.clone(),
                content: Arc::from(content.clone()),
            },
            content.len(),
            false,
        ),
        RemoteToolOutput::Failure(error) => (
            Event::ToolFailed {
                agent: pending.agent.clone(),
                epoch: pending.epoch,
                call_id: pending.call_id.clone(),
                error: Arc::from(error.clone()),
            },
            error.len(),
            true,
        ),
    };
    ctx.emit(
        &pending.agent,
        RunnerEvent::ToolExecuted {
            call_id: pending.call_id,
            tool: pending.request.tool,
            output_len,
            is_error,
        },
    );
    Ok(runner::resume(session, ctx, event))
}

/// 中止 Web 宿主尚未完成的调用，并把取消事件送回同一条事件泵。
///
/// actor 处理 `Cancel` 时既已翻转共享取消标记，又会调用此函数，因此等待 Web
/// 回传的空闲会话也能立即结束；迟到结果会因等待槽已清空被安全拒绝。
pub fn cancel_pending_remote_tools(session: &mut Session, ctx: &mut RunnerCtx) -> TurnStatus {
    ctx.discard_remote_tools();
    runner::resume(session, ctx, Event::Cancel { agent: session.agent().clone() })
}
