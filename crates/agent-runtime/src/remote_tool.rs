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
use crate::runner_entry;
use crate::transient_source_failure::TransientSourceFailure;
use crate::{RemoteToolTerminalOrigin, RemoteToolTerminalStatus};

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
            self.agent.as_str(),
            self.call_id.0
        )
    }
}

impl std::error::Error for RemoteToolResultError {}

/// A remote-tool continuation either rejected the host callback or exposed the original
/// transient-source provider failure.
#[derive(Debug)]
pub enum ResolveRemoteToolError {
    InvalidResult(RemoteToolResultError),
    TransientSource(TransientSourceFailure),
}

/// 校验并消费一个等待中的远端工具调用，然后从该结果恢复事件泵。
///
/// 116：泵 async 化之后跟着变成 `async fn`——它内部调的 `runner::
/// resume_after_first_commit` 本身就是 await 链的一环，不是新增的等待。native 上
/// 的同步入口见 [`resolve_remote_tool`]。
pub async fn resolve_remote_tool_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: AgentId,
    call_id: ToolCallId,
    output: RemoteToolOutput,
) -> Result<TurnStatus, ResolveRemoteToolError> {
    if ctx.pending_remote_tools.pending.iter().any(|pending| {
        pending.agent == agent
            && pending.call_id == call_id
            && crate::transient_source_policy::is_transient_source(&pending.request.tool)
    }) {
        return Err(ResolveRemoteToolError::InvalidResult(
            RemoteToolResultError { agent, call_id },
        ));
    }
    let pending = ctx.take_remote_tool(&agent, &call_id).ok_or_else(|| {
        ResolveRemoteToolError::InvalidResult(RemoteToolResultError {
            agent: agent.clone(),
            call_id: call_id.clone(),
        })
    })?;
    let (event, output_len, is_error, terminal_status) = match output {
        RemoteToolOutput::Success(content) => (
            Event::ToolResult {
                agent: pending.agent.clone(),
                epoch: pending.epoch,
                call_id: pending.call_id.clone(),
                content: Arc::from(content.clone()),
            },
            content.len(),
            false,
            RemoteToolTerminalStatus::Succeeded,
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
            RemoteToolTerminalStatus::Failed,
        ),
    };
    let event_agent = pending.agent.clone();
    let event_call = pending.call_id.clone();
    let event_tool = pending.request.tool.clone();
    runner::resume_after_first_commit(session, ctx, event, move |ctx| {
        ctx.record_remote_tool_terminal(
            &pending,
            terminal_status,
            RemoteToolTerminalOrigin::Host,
            None,
            None,
        );
        ctx.emit(
            &event_agent,
            RunnerEvent::ToolExecuted {
                call_id: event_call,
                tool: event_tool,
                output_len,
                is_error,
            },
        );
    })
    .await
    .map_err(ResolveRemoteToolError::TransientSource)
}

/// [`resolve_remote_tool_async`] 的同步壳。理由与 `cfg` 的取舍见 `crate::runner`
/// 模块文档「但公开入口在 native 上仍然是同步的」。
#[cfg(not(target_arch = "wasm32"))]
pub fn resolve_remote_tool(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    agent: AgentId,
    call_id: ToolCallId,
    output: RemoteToolOutput,
) -> Result<TurnStatus, ResolveRemoteToolError> {
    crate::block_on(resolve_remote_tool_async(
        session, ctx, agent, call_id, output,
    ))
}

/// 中止 Web 宿主尚未完成的调用，并把取消事件送回同一条事件泵。
///
/// actor 处理 `Cancel` 时既已翻转共享取消标记，又会调用此函数，因此等待 Web
/// 回传的空闲会话也能立即结束；迟到结果会因等待槽已清空被安全拒绝。
///
/// 116：同上，`async fn` 只是跟着 `runner_entry::resume_async` 走。native 上的同步入口
/// 见 [`cancel_pending_remote_tools`]。
pub async fn cancel_pending_remote_tools_async(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<TurnStatus, TransientSourceFailure> {
    ctx.discard_remote_tools();
    runner_entry::resume_async(
        session,
        ctx,
        Event::Cancel {
            agent: session.agent().clone(),
        },
    )
    .await
}

/// [`cancel_pending_remote_tools_async`] 的同步壳。理由与 `cfg` 的取舍见
/// `crate::runner` 模块文档「但公开入口在 native 上仍然是同步的」。
#[cfg(not(target_arch = "wasm32"))]
pub fn cancel_pending_remote_tools(
    session: &mut Session,
    ctx: &mut RunnerCtx,
) -> Result<TurnStatus, TransientSourceFailure> {
    crate::block_on(cancel_pending_remote_tools_async(session, ctx))
}
