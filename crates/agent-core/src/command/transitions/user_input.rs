//! `Idle + UserInput`：转移表里唯一开启一轮的格子。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::event::UserImage;
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
    images: Vec<UserImage>,
    event_desc: &Arc<str>,
) -> Vec<Effect> {
    if !matches!(txn.status(), TurnStatus::Idle) {
        return protocol_violation(txn, event_desc);
    }
    let blocks = if images.is_empty() {
        // 没图的历史块必须保持旧路径的值和结构，防止文本会话的 prompt 发生任何漂移。
        vec![ContentBlock::Text(text)]
    } else {
        let mut blocks = Vec::with_capacity(images.len() + 1);
        blocks.push(ContentBlock::Text(text));
        // 这些块会进 prompt：文本永远在前，图片严格保持宿主给的 Vec 顺序，不能改成
        // 无序容器或在别处重排，否则相同输入的前缀字节会漂移（红线 11）。
        blocks.extend(images.into_iter().map(|image| ContentBlock::Image {
            reference: image.reference,
            mime: image.mime,
            name: image.name,
        }));
        blocks
    };
    txn.push_message(Role::User, blocks);
    // 016：即使是这一轮的第一次 CallProvider，也要过 `max_turns` 闸——`max_turns`
    // 为 0（古怪但合法的宿主配置）时第一次尝试就该被拒绝，不是只挡后续几次。
    try_call_provider(txn)
}
