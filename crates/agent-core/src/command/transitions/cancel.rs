//! `Event::Cancel`：016 验收原文「取消在任意状态下都生效」。
//!
//! 非终态（`Idle`/`Thinking`/`ToolsPending`）统一处理：bump epoch（红线 6——让在飞
//! 的一切失效）、发 `[CancelInFlight{旧 epoch}, Emit(TurnStatusChanged)]`、落
//! `Failed(Cancelled)`；`ToolsPending` 额外清空槽位（不清的话终态里留着一堆再也不会
//! 被回执认领的 `Pending` 槽，是自身即会误导的死数据）。三个状态共用同一条路径，
//! 不按状态分支：016 的验收原文本身就是「取消不看当前在干什么」。
//!
//! 终态（`Done`/`Failed`）：`ProtocolViolation`——没有任何东西在飞可取消。判它而不是
//! 静默 no-op，是刻意跟「过期 epoch 被闸挡掉」区分开的：过期 epoch 是**一定会发生的
//! 正常时序噪音**（取消之后一定有一批回执陆续到达），静默丢弃是对的；终态收到
//! `Cancel` 不是这种必然噪音，跟 002 判 `Done + UserInput` 非法是同一类问题。

use std::sync::Arc;

use crate::command::txn::Txn;
use crate::engine::effect::Effect;
use crate::engine::notice::Notice;
use crate::engine::state::{Failure, TurnStatus};

use super::protocol_violation;

pub(super) fn on_cancel(txn: &mut Txn, event_desc: &Arc<str>) -> Vec<Effect> {
    if txn.status().is_terminal() {
        return protocol_violation(txn, event_desc);
    }

    // `CancelInFlight` 带的是**旧** epoch：它说的是「把这一代发出去的东西都停掉」。
    // 新世代此刻还没开始，用它去取消一个没发生过的世代说不通。
    let cancelled_epoch = txn.epoch();
    txn.request_epoch_bump();
    txn.set_tool_slots(Vec::new());
    let status = TurnStatus::Failed(Failure::Cancelled);
    txn.set_status(status.clone());

    vec![
        Effect::CancelInFlight {
            epoch: cancelled_epoch,
        },
        Effect::Emit(Notice::TurnStatusChanged { status }),
    ]
}
