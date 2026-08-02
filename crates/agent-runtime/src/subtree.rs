//! 子 agent 的槽位记账：**哪个子 agent 对应父 agent 的哪个 spawn 槽**，
//! 以及子 agent 到终态之后那个槽收敛成什么。
//!
//! # 为什么结果回父是 tool_result，而不是一个 `ChildFinished` 事件
//!
//! 决策 20 / issue 006 的决策记录已经拍板：spawn 是一次 tool call，它的槽位天然
//! 走 `ToolsPending` 的收敛路径，所以「等所有子完成」不需要任何新机制——父的
//! 那几个槽位全部 `Finished` 就是「等到齐了」。001 当年推迟 `ChildFinished` 时
//! 的直觉（「未必长成一个事件」）在这里被验证为正确：这个文件把子 agent 的终态
//! 翻译成 `Event::ToolResult` / `Event::ToolFailed`，喂回去的是转移表已经有的那
//! 两格，`agent-core` 一行没加。
//!
//! # 也没有「等所有子完成」的汇聚 derived（029 §注意）
//!
//! 同一条理由：父的 spawn 槽位收敛**就是**等待语义。为它再建一个读遍所有子
//! `Status` 的 derived，等于给同一个问题准备两个可能对不上的答案——而红线 4 的
//! 孪生条款（汇聚 derived 必须按 `AgentId` 现查 family）之所以危险，正是因为
//! 那种 atom 一旦存在就会被到处引用。028 为它留的 `StillRead` 黑盒缺口
//! （despawn 撞上「仍被读依赖」那条分支的真实触发场景）因此顺延，如实记录。
//!
//! # 记的是 `AgentId` / `ToolCallId`，不是 `AtomId`（红线 4 孪生条款）
//!
//! 这张表跨越「起飞」和「落地」两个时刻，中间可能夹着 undo。存 `AtomId` 就是把
//! 一个只在进程内、只在这一版图上有效的号码缓存过一次回滚——查询一律拿
//! `AgentId` 现问 `Session`（`status_of` / `messages_of`），一次都不缓存。

use std::sync::Arc;

use agent_core::{
    AgentId, ContentBlock, Epoch, Event, Failure, Message, Role, Session, ToolCallId, TurnStatus,
};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::spawn_tool::SPAWN_TOOL;

/// 一个还没收敛的 spawn 槽。
struct ChildSlot {
    child: AgentId,
    parent: AgentId,
    call_id: ToolCallId,
    /// **spawn 那一刻的世代**，不是收敛那一刻的。父那个槽等的是这一代发出去的
    /// 那次调用；中间要是被取消/undo 推过世代，这条结果就该跟别的在飞回执一样
    /// 被 `Session::step` 的闸挡掉（红线 6）——用「现在的 epoch」交差等于绕过闸。
    epoch: Epoch,
}

/// 本轮所有在飞的子 agent。
#[derive(Default)]
pub(crate) struct Subtree {
    slots: Vec<ChildSlot>,
}

impl Subtree {
    /// 记一笔：`child` 干完了要去认领 `parent` 的 `call_id` 那个槽。
    pub(crate) fn record(&mut self, child: AgentId, parent: AgentId, call_id: ToolCallId, epoch: Epoch) {
        self.slots.push(ChildSlot { child, parent, call_id, epoch });
    }

    /// 收割：每个已经到终态的子 agent 各产出一条喂给**父** agent 的事件，并从
    /// 表里划掉。没到终态的原样留着。
    ///
    /// 一次收割可能同时产出多条（两个子 agent 在同一批事件里先后落终态），
    /// 顺序按记账顺序 = 模型请求 spawn 的顺序，确定。
    pub(crate) fn harvest(&mut self, session: &Session, ctx: &mut RunnerCtx) -> Vec<Event> {
        let mut events = Vec::new();
        let mut i = 0;
        while i < self.slots.len() {
            let status = session.status_of(&self.slots[i].child);
            if !status.is_terminal() {
                i += 1;
                continue;
            }
            let slot = self.slots.remove(i);
            let (content, is_error) = outcome(session, &slot.child, &status);
            ctx.emit(
                &slot.parent,
                RunnerEvent::ToolExecuted {
                    call_id: slot.call_id.clone(),
                    tool: Arc::from(SPAWN_TOOL),
                    output_len: content.len(),
                    is_error,
                },
            );
            events.push(if is_error {
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
            });
        }
        events
    }
}

/// 子 agent 的终态 → 回给父的那段文本 + 它算不算失败。
///
/// **`is_error` = 子 Failed**（029 原文）。`Done { truncated: true }` 不算失败：
/// 它撞的是轮数闸，手上已经有半份答案，那份答案比一句「失败了」有用得多——003
/// 的哲学跨 agent 版，让模型看到全貌自己判断。前面加一行固定的说明让它知道
/// 这份答案是被截断的（固定文本，不带任何时间/计数，红线 11）。
fn outcome(session: &Session, child: &AgentId, status: &TurnStatus) -> (String, bool) {
    match status {
        TurnStatus::Done { truncated: false } => (final_text(session, child), false),
        TurnStatus::Done { truncated: true } => (
            format!("[子 agent 撞到轮数上限，下面是它停下时的最后回复]\n{}", final_text(session, child)),
            false,
        ),
        TurnStatus::Failed(Failure::Cancelled) => ("子 agent 被取消，没有产出结果。".to_string(), true),
        TurnStatus::Failed(Failure::Provider(class)) => {
            (format!("子 agent 失败（provider {class:?}），没有产出结果。"), true)
        }
        // 泵只在终态才收割，非终态在这里是不可达的——但 `TurnStatus` 是公开枚举，
        // 用 `unreachable!` 换一句诚实的兜底文本：一条奇怪的 tool_result 比一次
        // panic 好，父 agent 至少还能继续。
        other => (format!("子 agent 停在非终态 {other:?}，没有产出结果。"), true),
    }
}

/// 子 agent 的最后一条 assistant 消息里的可见文本。
///
/// 只取 `Text` 块：`Thinking` 是它的思考过程（要不要进 prompt 是 adapter 的判断，
/// 不该由我们替父 agent 决定），`ToolUse` / `ToolResult` 是它的干活痕迹，父 agent
/// 要的是结论。一条消息里多个 `Text` 块按顺序换行拼接。
fn final_text(session: &Session, child: &AgentId) -> String {
    let messages = session.messages_of(child);
    let last = messages.iter().rev().find(|m| m.role == Role::Assistant && has_text(m));
    match last {
        Some(message) => message
            .blocks
            .iter()
            .filter_map(|b| match b {
                ContentBlock::Text(t) => Some(&**t),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => "（子 agent 没有产出任何文本）".to_string(),
    }
}

fn has_text(message: &Message) -> bool {
    message.blocks.iter().any(|b| matches!(b, ContentBlock::Text(_)))
}
