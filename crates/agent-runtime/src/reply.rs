//! 宿主侧截获的那几个工具（`srv:agent/spawn` / `srv:agent/status` /
//! `srv:agent/collect`）怎么把**当场就有的**结果交回泵。
//!
//! # 为什么值一个模块
//!
//! 「一次工具调用收尾」在这条路上恒是**两件必须成对发生的事**：给宿主发一条
//! [`RunnerEvent::ToolExecuted`]（人看得见这次调用完了、正文多长、成不成），
//! 以及给泵一条 `ToolResult` / `ToolFailed`（模型那边的槽位收敛）。漏掉前者不会
//! 报错、测试也照绿——只是 CLI/面板上这次调用永远停在「executing」。三个截获工具
//! 各写一遍这对动作，就是三处各自可能漏一半；写成一处，漏不了。
//!
//! `is_error` 与事件变体的对应也在这里定死一次：`ToolFailed` ⟺ `is_error: true`。
//! 两边分开写时，「通报说成功、事件发的是 ToolFailed」是个能编译过的组合。
//!
//! # 不管**没有**当场结果的那条路
//!
//! 前台 spawn（父那个槽保持 `Pending` 等子 agent）和 collect 绑定（同理）不经过
//! 这里：它们的收尾发生在子 agent 落终态那一刻，由 `crate::subtree` 的收割负责，
//! 那条路上同一对动作也是成对发出的（见 `Subtree::harvest_slots`）。

use std::sync::Arc;

use agent_core::{AgentId, Epoch, Event, ToolCallId};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;

/// 当场算出了结果：通报 + 收敛槽位。
///
/// `tool` 是 `&str` 不是 `&'static str`——146 起 `intercept_registry` 的适配器
/// 拿到的名字是装配期注册的 `Arc<str>`，没有 `'static` 生命周期；`settle` 内部
/// 只做一次 `Arc::from(tool)`，这个操作对任意生命周期的 `&str` 都成立，放宽签名
/// 对既有四条截获（都传 `&'static str` 常量）零影响，`&'static str` 天然满足
/// 更宽的 `&str`。
pub(crate) fn ok(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    tool: &str,
    body: String,
) -> Dispatched {
    settle(ctx, agent, call_id, epoch, tool, body, false)
}

/// 这次调用做不成：**`is_error` 的 tool_result 回给模型**（决策 20），不是 panic，
/// 也不是让这一轮卡住。父那个槽位照常收敛，loop 接着跑，模型看着这句话自己收敛。
pub(crate) fn refuse(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    tool: &str,
    message: String,
) -> Dispatched {
    settle(ctx, agent, call_id, epoch, tool, message, true)
}

/// 成败由调用方给（`collect` 领到的结果成不成，取决于**子 agent** 干成没有，
/// 不取决于 collect 这次调用本身）。
pub(crate) fn settle(
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    tool: &str,
    body: String,
    is_error: bool,
) -> Dispatched {
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuted {
            call_id: call_id.clone(),
            tool: Arc::from(tool),
            output_len: body.len(),
            is_error,
        },
    );
    let agent = agent.clone();
    Dispatched::Event(if is_error {
        Event::ToolFailed {
            agent,
            epoch,
            call_id,
            error: Arc::from(body),
        }
    } else {
        Event::ToolResult {
            agent,
            epoch,
            call_id,
            content: Arc::from(body),
        }
    })
}
