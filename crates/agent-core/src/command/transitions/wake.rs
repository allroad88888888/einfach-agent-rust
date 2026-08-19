//! [`Event::Wake`]：把一个已经落终态的 agent 在**同一个 turn 内**重新拉回泵里
//! （214，决策 35 §二）。
//!
//! # 它只做一件事：再发一次 `CallProvider`
//!
//! 话已经由 `Session::drain_now` 进了 `Messages`（206 的定点），这条转移**不
//! `push_message`**——两处都写就是同一句话进两次历史，而那不会报错，只会让模型
//! 看到自己被说了两遍。
//!
//! # 三个答错都不报错的问题（214 §三）
//!
//! **一、属于哪个 turn？** 当前这个，**不开新 turn**。于是 `turn_id` 继承、undo
//! 连带子树、`Subtree` 的局部绑定全部一行不改（ORCHESTRATION §二 的既有结论）。
//! 开新 turn 就是「跨 turn 复活已死的子 agent」——决策 204 §五 明确不做。
//!
//! **二、`TurnsUsed` 怎么算？** 照常计数，**绝不重置**：这里走的是
//! [`try_call_provider`](super::try_call_provider) 那条共用出口，跟别的调用一视同仁。
//! 重置那条路是 `begin_turn` 的（`Txn::clear_turn_budget`），唤醒不碰它。
//!
//! 这是这一波唯一会**静默出错**的地方：写成重置，两个 agent 互相喊话就是真无界
//! ——不报错、测试也不红，只把 token 烧到见底。
//!
//! **三、撞顶了怎么办？** **不唤醒，什么都不写**，条目留在收件箱里，落回 206 §3
//! 的行为（轮末由 `agent_runtime::unread_inbox` 告警）。
//!
//! # 211 把入口从「终态」放宽到「没在跑」
//!
//! 自驱动的一轮（`agent_runtime::auto_turn`）是三步：`begin_turn`、
//! `drain_next_turn`、这条转移。`begin_turn` 之后 root 是 `Idle`，而这一轮
//! **没有用户那句话可喂**——要发的料全在刚搬进 `Messages` 的那几条留言里。
//! 判据因此是「没在跑」而不是「已经跑完」，那本来就是这条转移的意思。
//!
//! 不能直接把这一格交给 `try_call_provider`：它撞顶时落 `Done{truncated:true}`，
//! 而这个 agent **已经是终态了**——再落一次终态没有意义，还会把「因为预算耗尽而
//! 没被叫醒」和「自己正常答完了」两件事在状态上抹平。所以闸在调用之前自己问一次。
//!
//! 不写 primitive ⇒ `Txn` 收上来的 `changes` 是空的 ⇒ `History::append` 拒绝空步
//! ⇒ 不留 entry。跟非法格是同一条结构事实。

use std::sync::Arc;

use crate::engine::effect::Effect;
use crate::engine::state::TurnStatus;

use super::super::txn::Txn;
use super::{protocol_violation, try_call_provider};

/// 入口是**没在跑的两态**：`Idle` 与终态（`Done`/`Failed`）。在飞的两态
/// （`Thinking` / `ToolsPending`）一律 `protocol_violation`。
///
/// 「还在跑的 agent 收到 Wake」不是「顺便也行」：它下一次请求本来就会带上那条话
/// （206 的定点在 `CallProvider` 派发处），这里再推一次就是同一轮里并排两次
/// provider 调用。
///
/// **`Idle` 那一支是 211 加的**（自驱动轮次）：`begin_turn` 之后 root 是 `Idle`，
/// 而一轮自开的轮次没有用户那句话可喂——它要发的请求，料全在
/// `drain_next_turn` 刚搬进 `Messages` 的那几条留言里。这条判据因此从「终态」
/// 放宽成「**没在跑**」，而那本来就是这条转移的意思：用你历史里已经有的东西
/// 再发一次请求。
///
/// 放宽的**不是** `on_user_input` 的闸（214 §缘起 点名不许动的那一处）：Wake
/// 不 `push_message`、也不开新 turn，「一轮从哪开始」仍然只由
/// `Session::begin_turn` 一处回答。
///
/// # 空历史不叫醒
///
/// `Messages` 空着说明没有任何可发的料——那种 Wake 只会让 adapter 收到一份
/// 没有消息的请求体。它不是协议违规（发的人没做错什么），但也没有任何事可做，
/// 所以跟撞顶一样：什么都不写、不留 entry。
pub(super) fn on_wake(txn: &mut Txn, event_desc: &Arc<str>) -> Vec<Effect> {
    let status = txn.status();
    if !status.is_terminal() && status != TurnStatus::Idle {
        return protocol_violation(txn, event_desc);
    }
    if txn.turns_exhausted() || txn.messages().is_empty() {
        return Vec::new();
    }
    try_call_provider(txn)
}
