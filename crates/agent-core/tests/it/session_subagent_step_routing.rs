//! 028：`step` 长出 agent 维度——事件的 `agent` 字段真正路由。
//!
//! 验收对应：「各 agent 的 turn 状态独立（status/槽位/预算 per-agent），epoch 仍是
//! 会话级」「`turn_id` 由 root 铸造，子 agent 的 entry 继承」。

mod support;

use std::sync::Arc;

use agent_core::{AgentId, ChildConfig, Effect, Session, Slot, TurnStatus};
use support::{provider_done_tool_use_for, tool_result_for, user_input_event, user_input_for};

fn cfg() -> ChildConfig {
    ChildConfig {
        tools_allowed: vec![Arc::from("srv:fs/read")],
    }
}

fn status_of(s: &Session, agent: &AgentId) -> TurnStatus {
    s.read_descendant(&AgentId::root(), agent, Slot::Status)
        .unwrap()
        .as_status()
        .unwrap()
        .clone()
}

/// 喂给子 agent 的事件只动子 agent 的槽位，root 一动不动。
#[test]
fn an_event_routes_to_the_agent_it_names() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();

    let effects = s.step(user_input_for(&child, "子任务"));

    assert_eq!(status_of(&s, &child), TurnStatus::Thinking);
    assert_eq!(s.status(), TurnStatus::Idle, "root 没被动过");
    assert_eq!(s.messages().len(), 0, "消息进的是子的历史，不是 root 的");
    assert!(matches!(
        effects.as_slice(),
        [Effect::Emit(_), Effect::CallProvider { agent, .. }] if agent == &child
    ));
}

/// 两个 agent 各跑各的轮：状态、消息、预算三样都是 per-agent 的槽位。
#[test]
fn each_agent_keeps_its_own_turn_state() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();
    let a1 = s.spawn_child(&root, cfg()).unwrap();
    let a2 = s.spawn_child(&root, cfg()).unwrap();

    let _ = s.step(user_input_event("root 自己也在跑"));
    let _ = s.step(user_input_for(&a1, "a1 的活"));
    let _ = s.step(provider_done_tool_use_for(
        &a1,
        s.epoch(),
        &[("c1", "srv:fs/read")],
    ));

    assert_eq!(s.status(), TurnStatus::Thinking);
    assert_eq!(status_of(&s, &a1), TurnStatus::ToolsPending);
    assert_eq!(
        status_of(&s, &a2),
        TurnStatus::Idle,
        "没被喂过事件的 agent 停在起点"
    );

    // a1 的工具槽收敛之后只有 a1 变，root 和 a2 不变。
    let _ = s.step(tool_result_for(&a1, s.epoch(), "c1", "结果"));
    assert_eq!(
        status_of(&s, &a1),
        TurnStatus::Thinking,
        "收敛后 a1 自己发下一次调用"
    );
    assert_eq!(s.status(), TurnStatus::Thinking);
    assert_eq!(status_of(&s, &a2), TurnStatus::Idle);
}

/// 预算是 per-agent 的：root 用掉的轮数不算在子头上。
#[test]
fn the_turn_budget_is_per_agent() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();

    let _ = s.step(user_input_event("root"));
    assert_eq!(s.turns_used(), 1);

    let _ = s.step(user_input_for(&child, "child"));
    assert_eq!(s.turns_used(), 1, "root 的计数没被子 agent 推进");
}

/// `turn_id` 只在 root 铸造：子 agent 的每条 entry 都继承所在 root turn 的号
/// （决策 5），于是 `undo_turn` 才能一次退回整轮连带子树。
///
/// 顺带钉住 `begin_turn` 是 **root 专属**的：它翻的是会话的页，子 agent 没有
/// 对应命令（它们的轮状态出生于 `spawn_child`）。所以第二轮里喂给子的事件必须
/// 是它当前状态接得住的那一格——不是「再来一次 UserInput」。
#[test]
fn every_entry_inherits_the_root_minted_turn_id() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "第一轮的活"));

    s.begin_turn();
    assert_eq!(s.turn_id(), 2);
    assert_eq!(
        status_of(&s, &child),
        TurnStatus::Thinking,
        "root 翻页不动子的轮状态"
    );
    let _ = s.step(support::provider_done_end_turn_for(
        &child,
        s.epoch(),
        "干完了",
    ));

    let turns: Vec<u64> = s.history().entries().map(|e| e.meta.turn_id).collect();
    assert_eq!(turns.first(), Some(&1), "spawn 落在第一轮");
    assert_eq!(turns.last(), Some(&2), "第二轮里子 agent 的 entry 是 2");
    assert!(turns.windows(2).all(|w| w[0] <= w[1]), "turn 号单调");
}

/// epoch 仍然是**会话级**的：子 agent 的取消 bump 的是整个会话的世代，
/// 于是 root 那一侧在飞的回执也一起作废（`Effect::CancelInFlight` 没有 agent 字段）。
#[test]
fn epoch_stays_session_wide() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();
    let _ = s.step(user_input_event("root 在飞"));
    let stale = s.epoch();
    let _ = s.step(user_input_for(&child, "子也在飞"));

    let effects = s.step(agent_core::Event::Cancel {
        agent: child.clone(),
    });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CancelInFlight { .. }))
    );
    assert_ne!(s.epoch(), stale, "取消 bump 的是会话的世代");

    // 旧世代的回执被 epoch 闸挡掉——不管它是谁的。
    let before = s.primitives();
    let _ = s.step(support::provider_done_end_turn_for(
        &AgentId::root(),
        stale,
        "幽灵",
    ));
    assert_eq!(s.primitives(), before);
}

/// 一个本会话里不存在的 agent：事件被丢，一个字节都没改。
#[test]
fn an_event_for_an_unknown_agent_is_dropped() {
    let mut s = Session::new(AgentId::root());
    let ghost = AgentId::root().child(7);
    let before = s.primitives();
    let len = s.history_len();

    let effects = s.step(user_input_for(&ghost, "我是谁"));

    assert!(effects.is_empty());
    assert_eq!(s.history_len(), len);
    assert_eq!(s.primitives(), before, "连一个 atom 都不该被建出来");
}
