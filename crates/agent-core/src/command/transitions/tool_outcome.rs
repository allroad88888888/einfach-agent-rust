//! `ToolsPending + ToolResult` / `ToolsPending + ToolFailed`：两条事件路径殊途同归，
//! 失败只是多标一个 `is_error: true`（003：部分失败不中止，模型比我们更知道这个
//! 失败要不要紧）。

use std::borrow::Cow;
use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::notice::Notice;
use crate::engine::state::{SlotState, ToolSlot, TurnStatus};
use crate::ids::ToolCallId;
use crate::limits::{DEFAULT_TOOL_OUTPUT_BYTES, truncate_tool_output, truncated_content_bytes};
use crate::value::message::{ContentBlock, Role};

use super::{protocol_violation, try_call_provider};

pub(super) fn on_tool_outcome(
    txn: &mut Txn,
    call_id: ToolCallId,
    content: Arc<str>,
    is_error: bool,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::ToolsPending) {
        return protocol_violation(txn, event_desc);
    }

    let slots = txn.tool_slots();
    let has_pending_slot = slots
        .iter()
        .any(|slot| slot.call_id == call_id && matches!(slot.state, SlotState::Pending));
    if !has_pending_slot {
        // 未知 `call_id`，或者这个槽已经落地过（重复回执）。两种情况都不是「等其余
        // 槽」——都是这条消息压根不该被当真，报违规而不是悄悄吞掉，也不是 panic：
        // 跟过期 epoch 不同，这不是正常的时序噪音。
        return protocol_violation(txn, event_desc);
    }

    let original_bytes = content.len() as u64;
    let truncated_view = truncate_tool_output(&content, DEFAULT_TOOL_OUTPUT_BYTES);
    let was_truncated = matches!(truncated_view, Cow::Owned(_));
    let kept_bytes = truncated_content_bytes(&content, DEFAULT_TOOL_OUTPUT_BYTES) as u64;
    let stored: Arc<str> = Arc::from(truncated_view.into_owned());

    txn.set_tool_slots(finish_slot(&slots, &call_id, stored, is_error));
    // 屏障（020）：这一条 entry 记录的是一次不可逆操作的结果，undo 走到它要停下问人。
    // **在这里而不是派发时标记**：派发那一步没有写下任何「这次调用发生了」的源状态，
    // 回滚它不需要越过任何副作用；真正不能白回滚的是「结果已经落地」这一条。
    if txn.is_irreversible(&call_id) {
        txn.mark_barrier();
    }

    let mut effects = Vec::new();
    if was_truncated {
        effects.push(Effect::Emit(Notice::ToolOutputTruncated {
            call_id,
            original_bytes,
            kept_bytes,
        }));
    }

    if txn.tools_converged() {
        // 全部槽位按**顺序**（等于模型请求的顺序）拼成 `ContentBlock::ToolResult`
        // ——失败的槽也照进，003：部分失败不中止。
        let converged = txn.tool_slots();
        let blocks: Vec<ContentBlock> = converged
            .iter()
            .map(|slot| match &slot.state {
                SlotState::Finished { content, is_error } => ContentBlock::ToolResult {
                    id: slot.call_id.clone(),
                    content: content.clone(),
                    is_error: *is_error,
                },
                SlotState::Pending => unreachable!("tools_converged 保证没有 Pending 槽"),
            })
            .collect();
        txn.set_tool_slots(Vec::new());
        txn.push_message(Role::Assistant, blocks);
        // 016：收敛之后想接着调 provider，但 `max_turns` 可能已经撞顶——
        // `try_call_provider` 统一处理「真发 CallProvider」还是「落 Done{truncated:true}」。
        effects.extend(try_call_provider(txn));
    }
    // 未收敛：状态不变（除了刚落地的那个槽），`effects` 最多只装着上面那条截断通报。
    // **这不是隐式忽略**——是「还有槽位在 Pending，等着」，003 的边界。

    effects
}

/// 把一个槽位标记为完成，返回新的槽位列表。
///
/// 调用前已经确认过目标槽是 `Pending`（是否合法是转移表的判断，这里只做机械重写）。
/// 重写整份列表而不是原地改一个元素：槽位整体是**一个** primitive
/// （`Slot::ToolSlots`），`Change` 的 `prev`/`next` 因此是两份完整列表——但列表里
/// 每个元素的三个大字段都是 `Arc`，一份 N 槽的克隆是 N 次指针拷贝，undo 日志里存
/// 旧版本几乎零成本（红线 5）。
fn finish_slot(
    slots: &[ToolSlot],
    call_id: &ToolCallId,
    content: Arc<str>,
    is_error: bool,
) -> Vec<ToolSlot> {
    slots
        .iter()
        .map(|slot| {
            if &slot.call_id == call_id && matches!(slot.state, SlotState::Pending) {
                ToolSlot {
                    state: SlotState::Finished { content: content.clone(), is_error },
                    ..slot.clone()
                }
            } else {
                slot.clone()
            }
        })
        .collect()
}
