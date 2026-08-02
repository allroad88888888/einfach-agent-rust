//! `Thinking + ProviderDone`：把回复落进历史，再按 `stop` 决定这一轮怎么走。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::notice::Notice;
use crate::engine::state::{Failure, SlotState, ToolSlot, TurnStatus};
use crate::ids::ToolCallId;
use crate::seam::{ErrorClass, PrefixImage};
use crate::value::message::{ContentBlock, Role};
use crate::value::session::{StopReason, TokenUsage};

use super::protocol_violation;

/// **落历史这一步无条件发生**，不管 `stop` 落进哪个分支——包括下面判定为
/// `ProtocolViolation`/`Failed` 的那几个：模型确实说了这些话，把它们记下来跟
/// 「这一步该怎么收尾」是两件独立的事（审计视角：即使响应自相矛盾或不可恢复，
/// 也要留下它原样说了什么）。
pub(super) fn on_provider_done(
    txn: &mut Txn,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
    usage: TokenUsage,
    mut prefix: PrefixImage,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::Thinking) {
        return protocol_violation(txn, event_desc);
    }

    // 拿到一个响应就是「provider 还活着」的证据，不管 `stop` 落进哪个分支——
    // 016：这次尝试的失败连续计数到此为止清零。整轮总共重试了几次不受影响，
    // 那是 `TurnsUsed` 管的事，两个计数器职责不同。
    txn.clear_retries();

    // `blocks` 马上要被整个 move 进消息历史，趁现在把 `ToolUse` 块的信息取出来
    // （`ToolCallId` / `Arc<str>` / `Arc<Value>` 全是指针拷贝，不重）。
    let tool_uses: Vec<(ToolCallId, Arc<str>, Arc<serde_json::Value>)> = blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolUse { id, name, input } => {
                Some((id.clone(), name.clone(), input.clone()))
            }
            _ => None,
        })
        .collect();

    txn.push_message(Role::Assistant, blocks);

    // `prompt_tokens` 用这次的真实 usage 回填——纯赋值不是判断（红线 12）。
    prefix.prompt_tokens = Some(usage.prompt);
    txn.set_prev_prefix(prefix);

    match stop {
        StopReason::EndTurn => done(txn, false),

        StopReason::ToolUse if !tool_uses.is_empty() => {
            let slots: Vec<ToolSlot> = tool_uses
                .into_iter()
                .map(|(call_id, name, input)| ToolSlot {
                    call_id,
                    tool: name,
                    input,
                    state: SlotState::Pending,
                })
                .collect();
            let dispatch: Vec<Effect> = slots
                .iter()
                .map(|slot| Effect::ExecuteTool {
                    agent: txn.agent().clone(),
                    call_id: slot.call_id.clone(),
                    tool: slot.tool.clone(),
                    input: slot.input.clone(),
                    epoch: txn.epoch(),
                })
                .collect();
            txn.set_tool_slots(slots);
            txn.set_status(TurnStatus::ToolsPending);

            let mut effects = vec![Effect::Emit(Notice::TurnStatusChanged {
                status: TurnStatus::ToolsPending,
            })];
            effects.extend(dispatch);
            effects
        }

        // `stop == ToolUse` 但一个 `ToolUse` 块都没有：响应自相矛盾（它自己说要用
        // 工具，却没给出任何调用）。016 的裁决：`ProtocolViolation`，不是 `Failed`
        // ——没有 `ProviderFailed` 事件，走的是成功路径，塞进 `Failed(Provider(_))`
        // 是在编一个 provider 没说过的错误分类。这里状态**不完全**不变：历史已经在
        // 上面无条件落地了，但 `status` 本身不动，留在 `Thinking`。`Cancel` 在任意
        // 非终态都生效，是天然的逃生舱。
        StopReason::ToolUse => protocol_violation(txn, event_desc),

        // `MaxTokens` 不是停止条件（016：要不要续写是策略不是终止），但响应确实被
        // 截断了，`Done { truncated: true }` 如实标记这一点。续写策略是 M2+。
        StopReason::MaxTokens => done(txn, true),

        // `StopSequence`：模型在配置好的停止点停下——**配置生效**，不是被打断，
        // 语义上等价于「答完了」，所以 `truncated` 是 `false`。
        StopReason::StopSequence => done(txn, false),

        // adapter 遇到没见过的 finish_reason 时如实存成 `Other`（不许猜成 `EndTurn`）。
        // 016 的裁决：`Failed(Provider(Unknown))`——不认识的 stop 当成功处理，会静默
        // 吞掉一段可能被截断/出错的回复。
        StopReason::Other(_reason) => {
            let status = TurnStatus::Failed(Failure::Provider(ErrorClass::Unknown));
            txn.set_status(status.clone());
            vec![Effect::Emit(Notice::TurnStatusChanged { status })]
        }
    }
}

/// 三个「这一轮到此为止」的分支共用的收尾。
fn done(txn: &mut Txn, truncated: bool) -> Vec<Effect> {
    let status = TurnStatus::Done { truncated };
    txn.set_status(status.clone());
    vec![Effect::Emit(Notice::TurnStatusChanged { status })]
}
