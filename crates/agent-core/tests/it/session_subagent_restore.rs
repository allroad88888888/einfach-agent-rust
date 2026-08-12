//! 028：崩溃恢复在多 agent 下仍然是「快照 + `apply_next`」。
//!
//! 「完整状态 = 所有 primitive atom 的值」这句话在多 agent 下要**双向**成立：
//! `primitives()` 出的是全树（family 遍历，天然如此），`restore()` 收的也必须是
//! 全树——落盘的键上带着 `AgentId`（红线 4 用逻辑键换来的红利），所以「当时有哪些
//! agent」不需要另存一份名单。少了这一步，多 agent 会话重启后子树整个消失，
//! 而快照里那些键会被当成「这一版 schema 不认识的键」报上来。

use crate::support;
use std::sync::Arc;

use crate::support::user_input_event;
use crate::support::user_input_for;
use agent_core::{AgentEntry, AgentId, AtomKey, ChildConfig, Session, Slot, TurnStatus};

fn cfg() -> ChildConfig {
    ChildConfig {
        tools_allowed: vec![Arc::from("srv:fs/read")],
        ..ChildConfig::default()
    }
}

/// 快照 + 日志重放之后，整棵树逐值相同，子 agent 还在活名单上。
#[test]
fn a_whole_tree_survives_a_snapshot_and_replay() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let _ = live.step(user_input_event("root 说话"));
    let child = live.spawn_child(&root, cfg(), None).unwrap();
    let _ = live.step(user_input_for(&child, "子干活"));
    let grandchild = live
        .spawn_child(&child, ChildConfig::default(), None)
        .unwrap();

    let snapshot = live.primitives();
    let entries: Vec<AgentEntry> = live.history().entries().cloned().collect();
    let cursor = live.cursor();
    let next_seq = entries.last().map_or(0, |e| e.seq + 1);

    let mut unknown = Vec::new();
    let restored = Session::restore(
        root.clone(),
        Some(snapshot.clone()),
        entries,
        cursor,
        next_seq,
        100,
        agent_core::AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .unwrap();

    assert!(
        unknown.is_empty(),
        "子 agent 的键不该被当成不认识的键：{unknown:?}"
    );
    assert_eq!(restored.primitives(), snapshot, "全树逐值相同");
    assert!(restored.is_live(&child) && restored.is_live(&grandchild));
    assert_eq!(restored.children_of(&root), vec![child.clone()]);
    assert_eq!(
        restored.live_agents(),
        vec![root.clone(), child.clone(), grandchild]
    );
    assert_eq!(
        restored
            .read_descendant(&root, &child, Slot::Status)
            .unwrap()
            .as_status()
            .unwrap(),
        &TurnStatus::Thinking
    );
}

/// 恢复出来的树是**活的**：接着喂事件照常转移，接着 undo 照常回滚。
#[test]
fn the_restored_tree_keeps_stepping_and_undoing() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = live.spawn_child(&root, cfg(), None).unwrap();
    let _ = live.step(user_input_for(&child, "干活"));

    let entries: Vec<AgentEntry> = live.history().entries().cloned().collect();
    let cursor = live.cursor();
    let next_seq = entries.last().map_or(0, |e| e.seq + 1);
    let mut restored = Session::restore(
        root.clone(),
        None,
        entries,
        cursor,
        next_seq,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .unwrap();

    assert_eq!(
        restored.primitives(),
        live.primitives(),
        "无快照的整份重放也是全树"
    );

    let _ = restored.step(support::provider_done_end_turn_for(
        &child,
        restored.epoch(),
        "完",
    ));
    assert_eq!(
        restored
            .read_descendant(&root, &child, Slot::Status)
            .unwrap()
            .as_status()
            .unwrap(),
        &TurnStatus::Done { truncated: false }
    );

    let _ = restored.undo_turn();
    assert!(
        !restored.is_live(&child),
        "退回 spawn 之前，子 agent 不在活名单上"
    );
}

/// 被 despawn 的子 agent 只留一个墓碑，快照里也就只有那一项——恢复之后它仍然
/// 是「不在活名单上」，而且它的号不会被再发一次。
#[test]
fn a_tombstone_survives_and_still_reserves_its_seq() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let first = live.spawn_child(&root, cfg(), None).unwrap();
    let _ = live.despawn_child(&first).unwrap();

    let snapshot = live.primitives();
    assert!(
        snapshot
            .iter()
            .any(|(k, _)| k == &AtomKey::Agent(first.clone(), Slot::ToolsAllowed))
    );

    let entries: Vec<AgentEntry> = live.history().entries().cloned().collect();
    let cursor = live.cursor();
    let next_seq = entries.last().map_or(0, |e| e.seq + 1);
    let mut restored = Session::restore(
        root.clone(),
        Some(snapshot),
        entries,
        cursor,
        next_seq,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .unwrap();

    assert!(!restored.is_live(&first));
    let second = restored.spawn_child(&root, cfg(), None).unwrap();
    assert_ne!(second, first, "重启之后也不会把用过的号再发一次");
    assert_eq!(second.as_str(), "root/a2");
}
