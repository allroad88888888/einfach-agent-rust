//! `Effect::ExecuteTool` 的 **MCP 第四路**执行点，拆成起飞（[`start`]）与落地
//! （[`finish`]）两半——跟 [`crate::provider_call`] 同款异步在飞机制，但落的是
//! **一条工具结果**（`ToolResult`/`ToolFailed`），不是模型响应。
//!
//! # 为什么异步（docs/MCP.md §「执行模型」）
//!
//! `tools/call` 是对子进程的 JSON-RPC 往返，可以任意慢（网络型 server）。同步在
//! actor 线程上跑完会冻住整棵 agent 树、undo/cancel 一起卡。所以起一个背景线程，
//! 泵（`crate::runner`）不被阻塞；结果回来经泵落地。
//!
//! # 红线 6 的回写点在哪
//!
//! 起飞时把当时的 `epoch` 存进 [`McpCall`]（在飞 credential），落地时 [`finish`]
//! 把它**原样盖回** `Event::ToolResult`/`ToolFailed`。真正的比对不在这里——在
//! `agent-core` 的 `Session::step` 入口那道 epoch 闸（`command/step.rs`）：调用在飞
//! 时用户 undo/cancel bump 了 epoch，结果回来 epoch 不符就被那道闸丢弃，不写进已
//! 回滚的世界。MCP **复用**那道闸，不新写一套。credential 的 epoch 是起飞那一刻的
//! 值，背景线程伪造不了它（线程只报内容，epoch 由泵手上的 credential 提供）。
//!
//! # 红线 3：活句柄不进 store
//!
//! [`start`] 只拿 `Arc<McpRegistry>`（store 外的进程内表）和从工具名解析出的 server
//! id；背景线程用 `with_client` 借出 client 跑一次阻塞往返——`McpClient` 句柄从不
//! 进任何 command / atom。锁只在这个背景线程上持住，actor/泵线程从不因此阻塞。
//!
//! # 背景线程之间也不互相挡（issue 070）
//!
//! 一棵 agent 树里可以同时有很多这样的背景线程（一轮并列多个工具调用、
//! `spawn(background)` 的多个子 agent 各自在调 MCP）。它们之间的排队粒度**由
//! `McpRegistry` 的两层锁决定，不在这一层**：打不同 server 的线程真并行，打同一个
//! server 的线程在那个 server 自己的锁上排队（一条 stdio 管道，应答靠 id 匹配，本来
//! 就必须串行）。070 之前这里是整张表一把锁，任意两个 MCP 调用互相挡，最长堵到超时。

use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;

use agent_core::{AgentId, Epoch, Event, ToolCallId};
use agent_mcp::{McpRegistry, flatten_tool_result};
use serde_json::Value;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::io_thread::IoMsg;

/// 一次在飞的 MCP 调用。**一个 agent 可以同时有多张**——一轮里模型可以并列发多个
/// 工具调用，所以 credential 的身份是 `(agent, call_id)`，不是光 `agent`
/// （provider 调用一个 agent 最多一张，那是 `Thinking` 状态的唯一性；工具槽不是）。
pub(crate) struct McpCall {
    pub(crate) agent: AgentId,
    /// 起飞时的世代。落地时原样带回事件里过闸（红线 6）。
    pub(crate) epoch: Epoch,
    pub(crate) call_id: ToolCallId,
    /// 工具全名（`mcp:<server>/<tool>`），只用于落地时那条 `ToolExecuted` 通报。
    pub(crate) tool: Arc<str>,
}

/// 起飞：解析 server id + 裸工具名 → 起一个背景线程跑阻塞 `tools/call` → 立刻返回
/// credential。**不等结果**（结果经 `tx` 回泵，落地由 [`finish`]）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn start(
    tx: SyncSender<IoMsg>,
    registry: Arc<McpRegistry>,
    agent: AgentId,
    call_id: ToolCallId,
    tool: Arc<str>,
    input: Arc<Value>,
    epoch: Epoch,
    timeout: Duration,
) -> McpCall {
    let (server_id, bare) = split_name(&tool);
    let arguments = (*input).clone();

    let thread_agent = agent.clone();
    let thread_call_id = call_id.clone();
    thread::spawn(move || {
        // server id 查不到（本 host 不可用 / 尚未连上 / 被摘掉重连中）→ 一条
        // is_error 结果，loop 照常继续（和 spawn refuse 同精神，不 panic 不卡死）。
        let outcome =
            registry.with_client(&server_id, |client| client.call(&bare, arguments, timeout));
        let (content, is_error) = match outcome {
            Some(Ok(value)) => {
                let out = flatten_tool_result(&value);
                (out.text, out.is_error)
            }
            Some(Err(err)) => (err.to_string(), true),
            None => (format!("MCP server '{server_id}' 不可用"), true),
        };
        let _ = tx.send(IoMsg::McpDone {
            agent: thread_agent,
            call_id: thread_call_id,
            content: Arc::from(content),
            is_error,
        });
    });

    McpCall {
        agent,
        epoch,
        call_id,
        tool,
    }
}

/// 落地：把背景线程报回的结果翻译成一个 loop 事件。**epoch 从 credential 原样盖回**
/// ——这就是红线 6 在 MCP 路上的回写点，比对交给 `Session::step` 的 epoch 闸。
pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    call: McpCall,
    content: Arc<str>,
    is_error: bool,
) -> Event {
    ctx.emit(
        &call.agent,
        RunnerEvent::ToolExecuted {
            call_id: call.call_id.clone(),
            tool: call.tool.clone(),
            output_len: content.len(),
            is_error,
        },
    );
    let McpCall {
        agent,
        epoch,
        call_id,
        ..
    } = call;
    if is_error {
        Event::ToolFailed {
            agent,
            epoch,
            call_id,
            error: content,
        }
    } else {
        Event::ToolResult {
            agent,
            epoch,
            call_id,
            content,
        }
    }
}

/// 从在飞表里摘走匹配 `(agent, call_id)` 的 credential；匹配不上（已被取消轮划掉
/// 或本就是迟到的重复回执）→ `None`，泵原地把这条 `McpDone` 丢掉。
pub(crate) fn take(
    calls: &mut Vec<McpCall>,
    agent: &AgentId,
    call_id: &ToolCallId,
) -> Option<McpCall> {
    let at = calls
        .iter()
        .position(|c| &c.agent == agent && &c.call_id == call_id)?;
    Some(calls.remove(at))
}

/// `mcp:<server>/<tool>` → `(server, bare_tool)`。截获闸（`crate::dispatch`）已经保证
/// 前缀是 `mcp:` 且工具表声明过它，这里只做纯拆分：剥掉 `mcp:` 再按第一个 `/` 切一
/// 刀。没有 `/`（畸形，正常到不了这）→ server 段为空，交给 `with_client` 查不到、
/// 落 is_error。
fn split_name(tool: &str) -> (String, String) {
    let rest = tool.strip_prefix("mcp:").unwrap_or(tool);
    match rest.split_once('/') {
        Some((server, bare)) => (server.to_string(), bare.to_string()),
        None => (String::new(), rest.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_name_peels_prefix_and_splits_on_first_slash() {
        assert_eq!(
            split_name("mcp:everything/echo"),
            ("everything".into(), "echo".into())
        );
        // 裸工具名里可以再有斜杠（只切第一刀）。
        assert_eq!(split_name("mcp:srv/a/b"), ("srv".into(), "a/b".into()));
    }
}
