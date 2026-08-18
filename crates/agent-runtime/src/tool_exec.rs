//! `Effect::ExecuteTool` 的执行点。M1 的内置工具是同步本地文件读——**不设
//! 超时**（012：「fs 读不会挂」，`srv:shell/exec` 自己的超时在 `agent-tools`
//! 内部处理，见 020），不起线程，直接在 actor 线程上跑完。
//!
//! **发起时快照（`ToolCallRequest`）由调用方（`runner::run_effect`）构造好传进来**
//! ——027 起它还要在派发前决定要不要 `Session::mark_no_undo`，那必须先看到
//! `reversibility`，两处各查一遍表没有意义，构造挪到调用方，这里只管执行。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, ToolCallId, ToolCallRequest};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;

pub(crate) fn execute(
    ctx: &mut RunnerCtx,
    agent: AgentId,
    call_id: ToolCallId,
    request: ToolCallRequest,
    epoch: Epoch,
) -> Event {
    ctx.emit(
        &agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request: request.clone(),
        },
    );

    match ctx.fs.execute(&request.tool, &request.input) {
        Ok(content) => {
            ctx.emit(
                &agent,
                RunnerEvent::ToolExecuted {
                    call_id: call_id.clone(),
                    tool: request.tool.clone(),
                    output_len: content.len(),
                    is_error: false,
                },
            );
            Event::ToolResult {
                agent,
                epoch,
                call_id,
                content: Arc::from(content),
            }
        }
        Err(err) => {
            let message = format!("[{}] {}", err.code, err.message);
            ctx.emit(
                &agent,
                RunnerEvent::ToolExecuted {
                    call_id: call_id.clone(),
                    tool: request.tool.clone(),
                    output_len: message.len(),
                    is_error: true,
                },
            );
            Event::ToolFailed {
                agent,
                epoch,
                call_id,
                error: Arc::from(message),
            }
        }
    }
}
