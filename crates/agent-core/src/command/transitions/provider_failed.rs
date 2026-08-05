//! `Event::ProviderFailed`：016 的错误分流。合法只在 `Thinking`——那是唯一存在
//! 「`CallProvider` 在飞」这件事的状态，其余四态收到它是协议违规。
//!
//! 分流规则：`Retryable` 且重试预算没耗尽 → 原状态重发 `CallProvider`
//! （[`retry_or_fail`]，同时被 [`super::timeout`] 的 provider 超时分支复用）；
//! 其余情况（`BadRequest`/`Auth`/`Exhausted`/`Unknown`，以及 `Retryable` 但预算
//! 已经耗尽）→ `Failed(Provider(class))`。`Exhausted` 因为不等于 `Retryable`，
//! 天然走不进重试分支——「永不重试」是分流条件本身的推论，不需要单开一条判断。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::notice::Notice;
use crate::engine::state::{Failure, TurnStatus};
use crate::seam::ErrorClass;

use super::protocol_violation;

pub(super) fn on_provider_failed(
    txn: &mut Txn,
    class: ErrorClass,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::Thinking) {
        return protocol_violation(txn, event_desc);
    }
    retry_or_fail(txn, class)
}

/// 重试判断的共用出口。调用方都已经在调用前确认过 `status == Thinking`，
/// 这里只管「重试还是放弃」。
///
/// **只在真的要发 `CallProvider` 时才报 `Notice::Retrying`**：重试预算够但
/// `max_turns` 已经撞顶（[`super::try_call_provider`] 内部的第二道闸）的话，这次
/// 「决定重试」并没有真的发生一次新的 provider 调用，喊「重试中」会跟紧跟着落地的
/// `Done{truncated:true}` 自相矛盾。
pub(super) fn retry_or_fail(txn: &mut Txn, class: ErrorClass) -> Vec<Effect> {
    if class == ErrorClass::Retryable && txn.record_retry_attempt() {
        let attempt = txn.retries_used();
        let max_retries = txn.max_retries();
        let mut effects = super::try_call_provider(txn);
        if matches!(effects.last(), Some(Effect::CallProvider { .. })) {
            effects.insert(
                0,
                Effect::Emit(Notice::Retrying {
                    attempt,
                    max_retries,
                }),
            );
        }
        return effects;
    }

    let status = TurnStatus::Failed(Failure::Provider(class));
    txn.set_status(status.clone());
    vec![Effect::Emit(Notice::TurnStatusChanged { status })]
}
