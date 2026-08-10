//! 028 独立测试：轮内 spawn 之后 `undo_turn` 的行为——issue 实做记录里那条
//! 「日志管值，不管驻留」裁决的黑盒验证（选「atom 留在图上，值回默认」，不是
//! 「连 atom 一起 despawn」）。
//!
//! 覆盖：undo 之后子从 `live_agents` 消失、事件被活性闸静默丢弃（无 effect、
//! 无新 entry、无 primitive 变化）、`primitives()` 里子的全部槽位一个不少（
//! 只是值回默认）——这一点是跟 despawn 墓碑语义（只剩 1 个）的关键区别，见
//! `subagent_indep_tombstone.rs`。以及：redo 之后整棵子树连带子写过的状态
//! 一起回来，而且还能接着工作。
//!
//! 黑盒来源：docs/issues/028-multi-agent-graph.md 「裁决：轮内 spawn 的子在
//! undo 之后是什么」+ 验收第 4 条、docs/STATE-MODEL.md §「子 agent」。

use crate::support::session::new_session;
use crate::support::{provider_done_end_turn_for, user_input_for};
use agent_core::{
    AgentId, AgentValue, AtomKey, ChildConfig, Session, Slot, TurnStatus, UndoReport,
};

fn child_slot_count(session: &Session, child: &AgentId) -> usize {
    session
        .primitives()
        .iter()
        .filter(|(k, _)| k.agent() == child)
        .count()
}

fn tools_allowed_of(session: &Session, child: &AgentId) -> AgentValue {
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(child.clone(), Slot::ToolsAllowed))
        .map(|(_, v)| v)
        .expect("ToolsAllowed atom 该在（哪怕是默认值）")
}

fn status_of(session: &Session, child: &AgentId) -> AgentValue {
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(child.clone(), Slot::Status))
        .map(|(_, v)| v)
        .expect("Status atom 该在")
}

#[test]
fn undoing_the_turn_that_spawned_a_child_removes_it_from_the_live_set() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
        )
        .expect("spawn child");

    assert!(session.is_live(&child));
    assert_eq!(session.live_agents(), vec![root.clone(), child.clone()]);

    // 子在同一轮里写状态。
    let before_write = session.history_len();
    session.step(user_input_for(&child, "hello from child"));
    assert!(
        session.history_len() > before_write,
        "子的 UserInput 该留一条 entry"
    );
    assert_eq!(child_slot_count(&session, &child), 18);

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "undo_turn 该 Applied，实际 {report:?}"
    );

    assert!(!session.is_live(&child), "撤回 spawn 之后子不该再活着");
    assert_eq!(session.live_agents(), vec![root.clone()]);
    assert!(session.children_of(&root).is_empty());

    // 裁决的核心：atom 还在（十八个槽位一个不少），只是值回默认。
    assert_eq!(
        child_slot_count(&session, &child),
        18,
        "undo 不逐出 atom，只回滚值——这是跟 despawn 墓碑语义的关键区别"
    );
    assert_eq!(tools_allowed_of(&session, &child), AgentValue::Null);
}

#[test]
fn events_for_an_undone_spawn_are_silently_dropped_by_the_liveness_gate() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default())
        .expect("spawn child");
    session.step(user_input_for(&child, "hi"));
    let undo = session.undo_turn();
    assert!(
        matches!(undo, UndoReport::Applied { .. }),
        "undo_turn 该 Applied，实际 {undo:?}"
    );
    assert!(!session.is_live(&child));

    let before_primitives = session.primitives();
    let before_history_len = session.history_len();

    // UserInput 的 epoch 恒为 None（026 判断），天然过 epoch 闸，落到活性闸上。
    let effects = session.step(user_input_for(&child, "are you still there?"));

    assert!(effects.is_empty(), "死 agent 的事件不该产生 effect");
    assert_eq!(
        session.history_len(),
        before_history_len,
        "死 agent 的事件不该落一条 entry"
    );
    assert_eq!(
        session.primitives(),
        before_primitives,
        "死 agent 的事件不该改动任何 primitive"
    );
}

#[test]
fn redo_brings_the_whole_subtree_back_and_it_keeps_working() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
        )
        .expect("spawn child");
    session.step(user_input_for(&child, "hello from child"));

    let undo = session.undo_turn();
    assert!(
        matches!(undo, UndoReport::Applied { .. }),
        "undo_turn 该 Applied，实际 {undo:?}"
    );
    assert!(!session.is_live(&child));

    let redo = session.redo_turn();
    assert!(
        matches!(redo, UndoReport::Applied { .. }),
        "redo_turn 该 Applied，实际 {redo:?}"
    );

    assert!(session.is_live(&child), "redo 之后子该活过来");
    assert_eq!(session.live_agents(), vec![root.clone(), child.clone()]);
    assert_ne!(
        tools_allowed_of(&session, &child),
        AgentValue::Null,
        "redo 之后工具子集该回到 spawn 时写入的那份"
    );
    assert_eq!(
        status_of(&session, &child),
        AgentValue::Status(TurnStatus::Thinking),
        "redo 也该把子写过的那条 UserInput 带回来（子处在 Thinking）"
    );

    // 子接着工作：它现在是 Thinking（redo 灌回了它写过的 UserInput），喂一条
    // 合法的 ProviderDone 让它推进到 Done，证明路由和转移表都在正常工作，
    // 不是仍然被当成死 agent 静默丢弃。
    let before_history_len = session.history_len();
    session.step(provider_done_end_turn_for(&child, session.epoch(), "答案"));

    assert!(
        session.history_len() > before_history_len,
        "redo 之后子必须能继续处理事件、继续记账"
    );
    assert_eq!(
        status_of(&session, &child),
        AgentValue::Status(TurnStatus::Done { truncated: false })
    );
}
