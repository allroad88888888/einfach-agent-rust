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
//! ## 那张表没有因为压缩变宽
//!
//! 105 给 `Event` 加了 `CompactDone` / `CompactFailed`，**它们不进这 35 格**：
//! 压缩跟轮状态正交（096 第六问定的判读时机是「turn 结束拿到 usage 时」，
//! 生效在下一轮出料单时），把它们摊进 5 态 × N 的网格只会得到五行同样的答案。
//!
//! 它们各自的一格：**过了 epoch 闸就发一条通报**
//! （`Notice::CompactionSummaryReceived` / `CompactionFailed`），**状态一个字节不写**
//! ——回写是 107。通报这一半是 105 就有的，它是红线 6 那道闸的可观测性：
//! 过期的回执静默丢弃、当代的回执说一声，两种结果因此分得出来。理由写在
//! [`transition`] 里那一格。
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
mod wake;

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
        // 摘要回执（105）。到这里的都已经过了 `Session::step` 的 epoch 闸，属于
        // 当前世代——**这一版就发一条通报，一个 primitive 都不写**。
        //
        // 发通报是 105 的一部分，不是 109 的预支：epoch 闸是个过滤器，而过滤器只有
        // **两种结果都可观测**才测得出来。过期的回执被静默丢弃（`step` 那边定的：
        // 取消/undo 之后一定有一批回执陆续到达，每条都喊一声只会刷屏），所以
        // 「接受」这一侧必须说话；两侧都不说话的话，一个「Compact 的回执一律丢弃」
        // 的实现跟正确实现在外面一模一样——正是红线 6 要防的那种静默。
        //
        // 不写状态是划给 107 的：
        //
        // - `CompactFailed` 这就是终局语义（106）：压缩这一次作废，边界不动，
        //   下一轮照常跑。它永远不写状态，不是等谁来填。
        // - `CompactDone` 的回写落在 107 的 `Session::apply_summary`（存正文 + 推
        //   边界 + 填引用，三件事一条 entry）。**107 落地之后这一格仍然不写状态**：
        //   回写要知道这次摘要盖住的 `upto`，而 105 定死了事件里不带它（effect 不带
        //   历史正文，事件也没有理由胖）。所以调用点在持有 `upto` 的那一方——它必须
        //   先把这条事件喂给 `step` 过闸、看到下面这条 `Notice` 之后才调
        //   `apply_summary`（契约与「绕开会怎样」写在 `command/apply_summary.rs`
        //   的模块文档里）。
        //
        // 不写 primitive ⇒ `Txn` 收上来的 `changes` 是空的 ⇒ `History::append`
        // 拒绝空步 ⇒ 不留 entry。跟非法格是同一条结构事实（通报本身不是状态）。
        Event::CompactDone { .. } => vec![Effect::Emit(Notice::CompactionSummaryReceived)],
        Event::CompactFailed { .. } => vec![Effect::Emit(Notice::CompactionFailed)],
        Event::Cancel { .. } => cancel::on_cancel(txn, &event_desc),
        // 唤醒（214）。**不进上面那张 5 态 × 7 的网格**，跟压缩那两条同一个理由：
        // 它只有一格合法（终态）、其余全是 `protocol_violation`，摊进网格只会得到
        // 五行里四行同样的答案。判据全在 `wake` 那个文件里。
        Event::Wake { .. } => wake::on_wake(txn, &event_desc),
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
        Event::CompactDone { .. } => "compact_done",
        Event::CompactFailed { .. } => "compact_failed",
        Event::Cancel { .. } => "cancel",
        Event::Wake { .. } => "wake",
    }
}

/// 非法组合的共用出口：状态不变，报一条 `Notice::ProtocolViolation`。
pub(super) fn protocol_violation(txn: &Txn, event_desc: &Arc<str>) -> Vec<Effect> {
    vec![Effect::Emit(Notice::ProtocolViolation {
        state: txn.status(),
        event: event_desc.clone(),
    })]
}

/// 合法但可能撞顶的共用出口：**想发一次 `CallProvider`**（新一轮或重试）。
/// `max_turns` 没到就落 `Thinking`、计数、发 `CallProvider`；到了就落
/// `Done{truncated:true}`。**五个**入口共用这一处（`user_input`、`tool_outcome`
/// 的收敛分支、`provider_failed`、`timeout` 的 provider 超时分支，以及 214 的
/// `wake`），散着写五份等于给这条闸开五个漏判的机会。
///
/// 214 的 `wake` 在调用它**之前**自己问一次 `Txn::turns_exhausted`：撞顶时它要的
/// 是「什么都不做」，不是这里的 `Done{truncated:true}`（那个 agent 已经是终态了，
/// 见 `wake` 模块文档）。那是一处刻意的例外，不是漏走这条出口。
///
/// 只在状态**真的变了**才发 `TurnStatusChanged`：重试路径是 `Thinking → Thinking`，
/// 没有变化，不该喊一声。
pub(super) fn try_call_provider(txn: &mut Txn) -> Vec<Effect> {
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
