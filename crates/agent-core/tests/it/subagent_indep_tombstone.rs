//! 028 独立测试：号不复用 + despawn 留下的墓碑。
//!
//! 黑盒来源：docs/issues/028-multi-agent-graph.md §「设计判断」3、验收
//! 「深度 4 / 子数 9 被拒」邻近的号复用段落、cargo doc 的 `command::spawn` /
//! `command::despawn` 模块文档。不读 `src/command/{spawn,despawn}.rs` 源码。

use crate::support::session::new_session;
use agent_core::{AgentValue, AtomKey, ChildConfig, Slot};

#[test]
fn despawning_and_respawning_does_not_reuse_the_dead_agents_number() {
    let mut session = new_session();
    let root = session.agent().clone();

    let first = session
        .spawn_child(&root, ChildConfig::default())
        .expect("spawn #1");
    let _report = session.despawn_child(&first).expect("despawn #1");

    let second = session
        .spawn_child(&root, ChildConfig::default())
        .expect("spawn #2");

    assert_ne!(first, second, "墓碑还在，号不能被回收");
    assert_eq!(second, root.child(2), "第二个号该往上取，不是回落到 1");
}

#[test]
fn a_dead_agents_tombstone_key_exists_with_null_but_is_live_says_no() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default())
        .expect("spawn");

    let before = session
        .primitives()
        .iter()
        .filter(|(k, _)| k.agent() == &child)
        .count();
    assert_eq!(
        before, 19,
        "每个 agent 一份 `Slot::ALL`（103 追加了 PrevSendPlan → 17，107 追加了 \
         Summaries → 18，134 追加了 PrefixChunks → 19）"
    );

    let report = session.despawn_child(&child).expect("despawn");
    assert_eq!(report.atoms_evicted, 18);
    assert!(!session.is_live(&child), "despawn 之后 is_live 该是假");

    let remaining: Vec<_> = session
        .primitives()
        .into_iter()
        .filter(|(k, _)| k.agent() == &child)
        .collect();

    assert_eq!(remaining.len(), 1, "只该剩 ToolsAllowed 这一个墓碑");
    assert_eq!(
        remaining[0].0,
        AtomKey::Agent(child.clone(), Slot::ToolsAllowed)
    );
    assert_eq!(remaining[0].1, AgentValue::Null);
}

/// 连续三代（spawn → despawn → spawn → despawn → …）在同一个父下面，号一路
/// 单调往上，没有任何一次复用。
#[test]
fn three_generations_under_the_same_parent_mint_three_distinct_numbers() {
    let mut session = new_session();
    let root = session.agent().clone();

    let mut ids = Vec::new();
    for _ in 0..3 {
        let id = session
            .spawn_child(&root, ChildConfig::default())
            .expect("spawn");
        let _report = session.despawn_child(&id).expect("despawn");
        ids.push(id);
    }

    assert_eq!(ids, vec![root.child(1), root.child(2), root.child(3)]);
    assert_ne!(ids[0], ids[1]);
    assert_ne!(ids[1], ids[2]);
    assert_ne!(ids[0], ids[2]);
}
