//! 转移表本体，原子图版：`(TurnStatus, Event)` → `(新状态写入, Vec<Effect>)`。
//!
//! **语义与 `engine::transitions` 逐格相同**（002 定形状、016 填满、003 收敛细节），
//! 差别只有一处：状态从平结构字段搬进了原子图，于是每一次写入都经 [`Txn`]
//! （→ `record_set` → 一条 `Change`）。**5 态 × 7 变体 = 35 格，10 格合法 /
//! 25 格非法**，零 `unimplemented!`：
//!
//! - **合法（10 格）**：`Idle+UserInput` / `Thinking+ProviderDone` /
//!   `ToolsPending+{ToolResult,ToolFailed}` / `{Idle,Thinking,ToolsPending}+Cancel` /
//!   `Thinking+ProviderFailed` / `{Thinking,ToolsPending}+Timeout`（provider 超时
//!   落 `Thinking`、工具超时落 `ToolsPending`，由 `call_id` 决定走哪条）。
//! - **非法（25 格）**：状态不变，`Emit(Notice::ProtocolViolation)`。含 016 的判断：
//!   终态收到 `Cancel` 也在这一类。
//!
//! 非法格**不写任何 primitive**，于是 `Txn` 收上来的 `changes` 是空的，
//! `History::append` 拒绝空步——「状态不变」在日志这一侧同样是结构事实：
//! 一次协议违规不会在 undo 栈里留下一个按下去没反应的幽灵步。
//!
//! ## 为什么这一份和 `engine/transitions/` 并存
//!
//! 027 把 runner / CLI 换接到 `Session` 之后，`engine::step` 那一路退役
//! （见 `docs/issues/027-cli-undo.md` 第 1 条）。在那之前两份并存，各自被一套
//! **等价重写**的测试钉着（对照表见 `docs/issues/026-state-into-atoms.md` 实做记录）
//! ——本 issue 不动 runtime / cli，而删掉旧路会当场打断它们。

use std::sync::Arc;

use crate::engine::effect::Effect;
use crate::engine::event::Event;
use crate::engine::notice::Notice;
use crate::engine::state::TurnStatus;

use super::txn::Txn;

mod cancel;
mod provider_done;
mod provider_failed;
mod timeout;
mod tool_outcome;
mod user_input;

/// 入口。**转移表唯一对外暴露的东西**——六个 `on_*` 处理器都是内部分支。
pub(super) fn transition(txn: &mut Txn, event: Event) -> Vec<Effect> {
    let event_desc: Arc<str> = Arc::from(format!("{event:?}"));

    match event {
        Event::UserInput { text, .. } => user_input::on_user_input(txn, text, &event_desc),
        Event::ProviderDone {
            blocks,
            stop,
            usage,
            prefix,
            ..
        } => provider_done::on_provider_done(txn, blocks, stop, usage, prefix, &event_desc),
        Event::ProviderFailed { class, .. } => {
            provider_failed::on_provider_failed(txn, class, &event_desc)
        }
        Event::ToolResult {
            call_id, content, ..
        } => tool_outcome::on_tool_outcome(txn, call_id, content, false, &event_desc),
        Event::ToolFailed { call_id, error, .. } => {
            tool_outcome::on_tool_outcome(txn, call_id, error, true, &event_desc)
        }
        Event::Timeout { call_id, .. } => timeout::on_timeout(txn, call_id, &event_desc),
        Event::Cancel { .. } => cancel::on_cancel(txn, &event_desc),
    }
}

/// 这一步在时间线上叫什么（`EntryMeta.label`）。按**事件种类**取，不按结果取：
/// 一条 entry 的 label 要能回答「当时发生了什么」，而不是「core 决定怎么办」——
/// 后者已经完整地记在 `changes` 里了。
pub(super) fn label_of(event: &Event) -> &'static str {
    match event {
        Event::UserInput { .. } => "user_input",
        Event::ProviderDone { .. } => "provider_done",
        Event::ProviderFailed { .. } => "provider_failed",
        Event::ToolResult { .. } => "tool_result",
        Event::ToolFailed { .. } => "tool_failed",
        Event::Timeout { .. } => "timeout",
        Event::Cancel { .. } => "cancel",
    }
}

/// 非法组合的共用出口：状态不变，报一条 `Notice::ProtocolViolation`。
fn protocol_violation(txn: &Txn, event_desc: &Arc<str>) -> Vec<Effect> {
    vec![Effect::Emit(Notice::ProtocolViolation {
        state: txn.status(),
        event: event_desc.clone(),
    })]
}

/// 合法但可能撞顶的共用出口：**想发一次 `CallProvider`**（新一轮或重试）。
/// `max_turns` 没到就落 `Thinking`、计数、发 `CallProvider`；到了就落
/// `Done{truncated:true}`。四个入口共用这一处（`user_input`、`tool_outcome` 的
/// 收敛分支、`provider_failed`、`timeout` 的 provider 超时分支），散着写四份等于
/// 给这条闸开四个漏判的机会。
///
/// 只在状态**真的变了**才发 `TurnStatusChanged`：重试路径是 `Thinking → Thinking`，
/// 没有变化，不该喊一声。
fn try_call_provider(txn: &mut Txn) -> Vec<Effect> {
    let prev = txn.status();

    if txn.record_turn_attempt() {
        txn.set_status(TurnStatus::Thinking);
        let mut effects = Vec::new();
        if prev != TurnStatus::Thinking {
            effects.push(Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::Thinking,
            }));
        }
        effects.push(Effect::CallProvider {
            agent: txn.agent().clone(),
            epoch: txn.epoch(),
        });
        effects
    } else {
        let status = TurnStatus::Done { truncated: true };
        txn.set_status(status.clone());
        vec![Effect::Emit(Notice::TurnStatusChanged { status })]
    }
}
