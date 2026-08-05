//! 028：`despawn_child` —— 019 三条硬约束在公开面上的样子。
//!
//! 「还有外部读者就整条拒绝」和「逐出顺序反了会被引擎拒」两条要握着 `Session`
//! 的内脏才测得出，住在 `src/command/despawn.rs` 的单元测试里。这里测的是三条
//! 约束里能从外面看见的部分，加上几种拒绝路径。

mod support;

use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, ChildConfig, DespawnRefused, Session, Slot};
use support::user_input_for;

fn cfg() -> ChildConfig {
    ChildConfig { tools_allowed: vec![Arc::from("srv:fs/read")] }
}

/// **019 硬约束 3**：teardown 把活值记成 `prev`，一条 entry 一次记完整棵子树。
#[test]
fn the_teardown_entry_carries_every_live_value_as_prev() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "干活"));

    let live: Vec<(AtomKey, AgentValue)> = s
        .primitives()
        .into_iter()
        .filter(|(k, _)| k.agent() == &child)
        .filter(|(k, v)| v != &default_of(k))
        .collect();
    assert!(live.len() >= 3, "至少 ToolsAllowed / Status / Messages 是非默认值");

    let _ = s.despawn_child(&child).unwrap();

    let entry = s.last_entry().unwrap();
    assert_eq!(entry.meta.label, "despawn_child");
    for (key, value) in &live {
        let change = entry
            .changes
            .iter()
            .find(|c| &c.key == key)
            .unwrap_or_else(|| panic!("{key:?} 的活值没被记成 prev —— undo 会拿回默认值"));
        assert_eq!(&change.prev, value);
        assert_eq!(change.next, default_of(key));
    }
}

fn default_of(key: &AtomKey) -> AgentValue {
    key.default_value()
}

/// **019 硬约束 1**：自叶向根、子树递归。报告里的顺序就是逐出的顺序。
#[test]
fn the_whole_subtree_comes_apart_leaf_first() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();
    let a1 = s.spawn_child(&root, cfg()).unwrap();
    let a1_a1 = s.spawn_child(&a1, cfg()).unwrap();
    let a1_a1_a1 = s.spawn_child(&a1_a1, cfg()).unwrap();

    let report = s.despawn_child(&a1).unwrap();
    assert_eq!(report.agents, vec![a1_a1_a1.clone(), a1_a1.clone(), a1.clone()]);
    // 每个 agent 留一个 `ToolsAllowed` 墓碑（号不复用 + 它是活名单）。
    assert_eq!(report.atoms_evicted, 3 * (Slot::ALL.len() - 1));

    for agent in [&a1, &a1_a1, &a1_a1_a1] {
        assert!(!s.is_live(agent));
        let left: Vec<AtomKey> = s
            .primitives()
            .into_iter()
            .map(|(k, _)| k)
            .filter(|k| k.agent() == agent)
            .collect();
        assert_eq!(left, vec![AtomKey::Agent((*agent).clone(), Slot::ToolsAllowed)]);
    }
    assert_eq!(s.live_agents(), vec![root]);
}

/// 只拆指定的那一支，兄弟不受影响。
#[test]
fn a_sibling_subtree_is_untouched() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();
    let a1 = s.spawn_child(&root, cfg()).unwrap();
    let a2 = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&a2, "兄弟还在干活"));

    let before: Vec<_> = s.primitives().into_iter().filter(|(k, _)| k.agent() == &a2).collect();
    let _ = s.despawn_child(&a1).unwrap();
    let after: Vec<_> = s.primitives().into_iter().filter(|(k, _)| k.agent() == &a2).collect();

    assert_eq!(before, after);
    assert!(s.is_live(&a2));
    assert_eq!(s.children_of(&root), vec![a2]);
}

/// 三种拒绝：root / 别的树 / 已经死了的。都返回错误值，不 panic。
#[test]
fn root_strangers_and_the_already_dead_are_refused() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();

    assert_eq!(s.despawn_child(&AgentId::root()), Err(DespawnRefused::Root));

    let alien = AgentId::new("other/a1");
    assert_eq!(
        s.despawn_child(&alien),
        Err(DespawnRefused::NotInSession { agent: alien })
    );

    let never = AgentId::root().child(99);
    assert_eq!(
        s.despawn_child(&never),
        Err(DespawnRefused::NotLive { agent: never })
    );

    let _ = s.despawn_child(&child).unwrap();
    assert_eq!(
        s.despawn_child(&child),
        Err(DespawnRefused::NotLive { agent: child })
    );
}

/// despawn 之后子 agent 的事件被静默丢弃——在飞回执是**正常现象**，
/// 跟 epoch 闸挡过期回执同源。
#[test]
fn events_for_a_despawned_child_are_dropped_silently() {
    let mut s = Session::new(AgentId::root());
    let child = s.spawn_child(&AgentId::root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "干活"));
    let _ = s.despawn_child(&child).unwrap();

    let before = s.primitives();
    let len = s.history_len();
    let effects = s.step(support::provider_done_end_turn_for(&child, s.epoch(), "太晚了"));

    assert!(effects.is_empty(), "不发 effect，也不发通报");
    assert_eq!(s.history_len(), len, "不落 entry");
    assert_eq!(s.primitives(), before, "一个 primitive 都没写");
}
