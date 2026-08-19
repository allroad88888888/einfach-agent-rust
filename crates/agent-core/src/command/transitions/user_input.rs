//! `Idle + UserInput`：转移表里唯一开启一轮的格子。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::state::TurnStatus;
use crate::value::message::{ContentBlock, Role};

use super::{protocol_violation, try_call_provider};

/// 别的状态收到 `UserInput` 都是非法——没有「排队等下一轮」，用户在轮子转起来
/// 之后再说话，那句话应该由宿主攒着（或者拒收）。**终态之后开新一轮走
/// [`Session::begin_turn`](crate::command::Session::begin_turn)**，不是靠这里
/// 对终态网开一面：那会把「一轮从哪开始」这个 turn 边界（`undo_turn` 的分组依据）
/// 藏进一格转移里。
pub(super) fn on_user_input(
    txn: &mut Txn,
    text: Arc<str>,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::Idle) {
        return protocol_violation(txn, event_desc);
    }
    txn.push_message(Role::User, vec![ContentBlock::Text(text)]);
    // 211：**这里是自驱动预算唯一被加满的地方**（决策 35 §二）。
    //
    // 「唯一」是承重的：按时间续期、按「有进展」续期、`begin_turn` 顺手重置，
    // 三种写法都是把闸接回被它约束的循环里 = 等于没有闸，而且不报错。
    // 判据是「**一句真实的用户输入**」——只有它证明还有人在看着。
    //
    // 位置在 `push_message` 之后、`try_call_provider` 之前无关紧要（两者都不读
    // 这个槽位），放这儿是为了让它挨着那句话：加满的理由就是那句话本身。
    txn.mark_refill_auto_turns();
    // 016：即使是这一轮的第一次 CallProvider，也要过 `max_turns` 闸——`max_turns`
    // 为 0（古怪但合法的宿主配置）时第一次尝试就该被拒绝，不是只挡后续几次。
    try_call_provider(txn)
}
