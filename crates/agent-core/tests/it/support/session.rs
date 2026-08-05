//! `Session` 侧的 fixture（026）。跟 `support/mod.rs` 一样只造输入，不含断言。
//!
//! **造状态只能走真实事件**：`Session` 不暴露 store，也就没有「直接把 status 赋成
//! `ToolsPending`」这种后门——M1 的 `state_with_status` 是靠给平结构字段赋值造出来的，
//! 那条路在原子图版本里结构上不存在（红线 2）。所以这里的 fixture 是**驱动**出来的，
//! 顺带证明了「这些状态确实到得了」。

#![allow(dead_code)]

use agent_core::{AgentId, AgentValue, AtomKey, Epoch, Session, TurnStatus};

use super::{cancel_event, provider_done_end_turn, provider_done_tool_use, user_input_event};

pub fn new_session() -> Session {
    Session::new(AgentId::root())
}

/// M1 `turn_transitions_grid.rs::all_statuses()` 的同一批取值。
pub fn all_statuses() -> Vec<TurnStatus> {
    vec![
        TurnStatus::Idle,
        TurnStatus::Thinking,
        TurnStatus::ToolsPending,
        TurnStatus::Done { truncated: false },
        TurnStatus::Failed(agent_core::Failure::Cancelled),
    ]
}

/// 把一个新会话驱动到指定状态。`ToolsPending` 带一个 `call_1` 的 `Pending` 槽
/// ——跟 M1 的 fixture 同形，好让合法分支与非法分支共用它。
pub fn session_at(status: &TurnStatus) -> Session {
    let mut session = new_session();
    match status {
        TurnStatus::Idle => {}
        TurnStatus::Thinking => {
            let _ = session.step(user_input_event("你好"));
        }
        TurnStatus::ToolsPending => {
            let _ = session.step(user_input_event("你好"));
            let _ = session.step(provider_done_tool_use(
                session.epoch(),
                &[("call_1", "srv:fs/read")],
            ));
        }
        TurnStatus::Done { .. } => {
            let _ = session.step(user_input_event("你好"));
            let _ = session.step(provider_done_end_turn(session.epoch(), "答案"));
        }
        TurnStatus::Failed(_) => {
            let _ = session.step(cancel_event());
        }
    }
    assert_eq!(&session.status(), status, "fixture 没能驱动到目标状态");
    session
}

/// 驱动到 `Thinking`（一条用户消息已经进历史，`turns_used == 1`）。
pub fn thinking_session() -> Session {
    session_at(&TurnStatus::Thinking)
}

/// 驱动到 `ToolsPending`，槽位就是 `calls` 给的那几个（顺序 = 模型请求顺序）。
pub fn session_with_pending_tools(calls: &[(&str, &str)]) -> Session {
    let mut session = thinking_session();
    let _ = session.step(provider_done_tool_use(session.epoch(), calls));
    assert_eq!(session.status(), TurnStatus::ToolsPending);
    assert_eq!(session.tool_slots().len(), calls.len());
    session
}

/// 会话的**全部**可观测状态：所有 primitive 的值 + 三个不住在图里的会话字段
/// + 日志长度。
///
/// 「非法转移不该改状态」在 M1 是 `assert_eq!(st, before)`（整个平结构逐字段比）。
/// 原子图版本的同一句话就是这个：primitive 一个不差（这就是「完整状态」的定义），
/// 外加 epoch / turn_id / 日志没有多出一条 entry——最后一条是 M1 没有的那一半：
/// 状态不变的转移**不该在 undo 栈里留下按下去没反应的幽灵步**。
#[derive(PartialEq, Debug)]
pub struct Observed {
    pub primitives: Vec<(AtomKey, AgentValue)>,
    pub epoch: Epoch,
    pub turn_id: u64,
    pub history_len: usize,
    pub cursor: usize,
}

pub fn observe(session: &Session) -> Observed {
    Observed {
        primitives: session.primitives(),
        epoch: session.epoch(),
        turn_id: session.turn_id(),
        history_len: session.history_len(),
        cursor: session.cursor(),
    }
}
