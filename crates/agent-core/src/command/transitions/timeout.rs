//! `Event::Timeout`：provider 超时（`call_id: None`）和工具超时（`call_id: Some(_)`）
//! 两条转移路径共用一个入口——决定走哪条的是事件自己携带的 `call_id`，不是当前状态，
//! 所以在 `transitions/mod.rs` 只按事件种类分发到这里之后，这里再按 `call_id` 二次分派。
//!
//! provider 超时按 016 验收原文「`call_id=None` 按 Retryable 走同一条重试路」，复用
//! [`super::provider_failed::retry_or_fail`]；工具超时按「`Some(id)` → 那个槽落
//! `Finished{is_error:true}`，收敛逻辑照旧」，直接复用
//! [`super::tool_outcome::on_tool_outcome`]——超时也是一条结果（003 的部分失败语义），
//! `on_tool_outcome` 本来就不关心「为什么失败」，只要给它 `is_error:true` 和一段文本。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::state::TurnStatus;
use crate::ids::ToolCallId;
use crate::seam::ErrorClass;

use super::{protocol_violation, provider_failed, tool_outcome};

/// 超时的占位文案。这段文字会进喂给模型的消息内容，必须逐字节确定（红线 11）
/// ——不带时间戳、不带具体等了多久，只说清「超时」这一个事实。
const TOOL_TIMEOUT_MESSAGE: &str = "工具执行超时，未在预期时间内返回结果。";

pub(super) fn on_timeout(
    txn: &mut Txn,
    call_id: Option<ToolCallId>,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    match call_id {
        None => on_provider_timeout(txn, event_desc),
        Some(id) => tool_outcome::on_tool_outcome(
            txn,
            id,
            Arc::from(TOOL_TIMEOUT_MESSAGE),
            true,
            event_desc,
        ),
    }
}

/// provider 超时：只在 `Thinking`（provider 调用在飞的唯一状态）合法，按
/// `Retryable` 走跟 `ProviderFailed` 一样的重试判断。
fn on_provider_timeout(txn: &mut Txn, event_desc: &Arc<str>) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::Thinking) {
        return protocol_violation(txn, event_desc);
    }
    provider_failed::retry_or_fail(txn, ErrorClass::Retryable)
}
