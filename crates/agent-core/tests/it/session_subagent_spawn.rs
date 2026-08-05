//! 028：`spawn_child` 的记账与两道结构性硬闸。
//!
//! 验收对应：
//! - 「spawn 记账：spawn 后 history 多一条 entry，changes 含子的初始槽位」
//! - 「深度 4 / 子数 9 被拒：`is_error` 语义的错误返回，不 panic」

mod support;

use std::sync::Arc;

use agent_core::{
    AgentId, AgentLimits, AgentValue, AtomKey, ChildConfig, Session, Slot, SpawnRefused,
};

fn cfg(tools: &[&str]) -> ChildConfig {
    ChildConfig {
        tools_allowed: tools.iter().map(|t| Arc::from(*t)).collect(),
    }
}

fn root() -> AgentId {
    AgentId::root()
}

#[test]
fn a_spawn_lands_exactly_one_entry_carrying_the_childs_initial_slot() {
    let mut s = Session::new(root());
    let before = s.history_len();

    let child = s.spawn_child(&root(), cfg(&["srv:fs/read"])).unwrap();

    assert_eq!(s.history_len(), before + 1, "spawn 恰好落一条 entry");
    let entry = s.last_entry().unwrap();
    assert_eq!(entry.meta.label, "spawn_child");
    assert_eq!(
        entry.meta.turn_id,
        s.turn_id(),
        "子 agent 的 entry 继承 root 的 turn_id"
    );

    let key = AtomKey::Agent(child.clone(), Slot::ToolsAllowed);
    let change = entry
        .changes
        .iter()
        .find(|c| c.key == key)
        .expect("changes 里含子的初始槽位");
    assert_eq!(change.prev, AgentValue::Null, "spawn 之前它不在活名单上");
    assert!(matches!(change.next, AgentValue::Json(_)));
}

#[test]
fn the_child_shows_up_on_the_tree_with_a_full_slot_table() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg(&[])).unwrap();

    assert_eq!(child.as_str(), "root/a1");
    assert!(s.is_live(&child));
    assert_eq!(s.children_of(&root()), vec![child.clone()]);
    assert_eq!(s.live_agents(), vec![root(), child.clone()]);

    // 与 root 同一条 `build_agent`：槽位一个不少（019 硬约束 1 的前提）。
    let mine = s
        .primitives()
        .into_iter()
        .filter(|(k, _)| k.agent() == &child)
        .count();
    assert_eq!(mine, Slot::ALL.len());
}

/// 深度闸：root 是 0，默认上限 3，所以第四层被拒——**返回错误值，不 panic**。
#[test]
fn depth_four_is_refused_with_a_value_not_a_panic() {
    let mut s = Session::new(root());
    let a1 = s.spawn_child(&root(), cfg(&[])).unwrap();
    let a2 = s.spawn_child(&a1, cfg(&[])).unwrap();
    let a3 = s.spawn_child(&a2, cfg(&[])).unwrap();
    assert_eq!(a3.depth(), 3);

    assert_eq!(
        s.spawn_child(&a3, cfg(&[])),
        Err(SpawnRefused::DepthExceeded { depth: 4, max: 3 })
    );
    // 被拒的那一下什么都没写。
    assert_eq!(s.children_of(&a3), Vec::<AgentId>::new());
}

/// 子数闸：默认上限 8，第九个被拒。
#[test]
fn the_ninth_child_is_refused() {
    let mut s = Session::new(root());
    for i in 1..=8 {
        let child = s.spawn_child(&root(), cfg(&[])).unwrap();
        assert_eq!(child.as_str(), format!("root/a{i}"));
    }
    let len = s.history_len();

    assert_eq!(
        s.spawn_child(&root(), cfg(&[])),
        Err(SpawnRefused::TooManyChildren { live: 8, max: 8 })
    );
    assert_eq!(s.history_len(), len, "被拒的 spawn 不留 entry");
    assert_eq!(s.children_of(&root()).len(), 8);
}

/// 子数上限数的是**活的**：despawn 一个就空出一格，而新的那个拿的是新号。
#[test]
fn despawning_frees_a_slot_but_not_the_seq() {
    let mut s = Session::new(root());
    let mut kids = Vec::new();
    for _ in 1..=8 {
        kids.push(s.spawn_child(&root(), cfg(&[])).unwrap());
    }
    let _ = s.despawn_child(&kids[0]).unwrap();

    let ninth = s.spawn_child(&root(), cfg(&[])).unwrap();
    assert_eq!(ninth.as_str(), "root/a9");
    assert_eq!(s.children_of(&root()).len(), 8);
}

#[test]
fn limits_are_parameters_and_can_be_dialed() {
    let mut s = Session::new(root());
    assert_eq!(
        s.agent_limits(),
        AgentLimits {
            max_depth: 3,
            max_children: 8
        }
    );

    s.set_agent_limits(AgentLimits {
        max_depth: 1,
        max_children: 1,
    });
    let a1 = s.spawn_child(&root(), cfg(&[])).unwrap();
    assert_eq!(
        s.spawn_child(&root(), cfg(&[])),
        Err(SpawnRefused::TooManyChildren { live: 1, max: 1 })
    );
    assert_eq!(
        s.spawn_child(&a1, cfg(&[])),
        Err(SpawnRefused::DepthExceeded { depth: 2, max: 1 })
    );
}

#[test]
fn a_dead_or_foreign_parent_is_refused() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg(&[])).unwrap();
    let _ = s.despawn_child(&child).unwrap();

    assert_eq!(
        s.spawn_child(&child, cfg(&[])),
        Err(SpawnRefused::ParentNotLive { parent: child })
    );
    let alien = AgentId::new("other/a1");
    assert_eq!(
        s.spawn_child(&alien, cfg(&[])),
        Err(SpawnRefused::NotInSession { parent: alien })
    );
}
