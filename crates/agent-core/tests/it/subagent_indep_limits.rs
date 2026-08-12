//! 028 独立测试：深度 / 子数结构性硬限（决策 20）——超限拒绝且不 panic，
//! `set_agent_limits` 不追溯。
//!
//! 黑盒来源：docs/ROADMAP.md 决策 20、docs/issues/028-multi-agent-graph.md
//! 验收「深度 4 / 子数 9 被拒」、cargo doc 的 `command::spawn` 模块文档
//! （`DEFAULT_MAX_AGENT_DEPTH = 3` / `DEFAULT_MAX_CHILDREN = 8`）、
//! `Session::set_agent_limits` 的文档注释（不追溯的理由）。不读
//! `src/command/spawn.rs` 源码。

use crate::support::session::new_session;
use agent_core::{AgentLimits, ChildConfig, SpawnRefused};

#[test]
fn depth_four_is_refused_without_panicking() {
    let mut session = new_session();
    let root = session.agent().clone();

    let mut current = root.clone();
    for _ in 0..3 {
        current = session
            .spawn_child(&current, ChildConfig::default(), None)
            .expect("depth <= 3 应该被允许");
    }
    assert_eq!(current.depth(), 3);

    let refused = session.spawn_child(&current, ChildConfig::default(), None);
    assert_eq!(
        refused,
        Err(SpawnRefused::DepthExceeded { depth: 4, max: 3 })
    );
    assert!(
        session.children_of(&current).is_empty(),
        "被拒的 spawn 不该留下任何痕迹"
    );
}

#[test]
fn the_ninth_sibling_is_refused_without_panicking() {
    let mut session = new_session();
    let root = session.agent().clone();

    for _ in 0..8 {
        session
            .spawn_child(&root, ChildConfig::default(), None)
            .expect("前 8 个该被允许");
    }
    assert_eq!(session.children_of(&root).len(), 8);

    let refused = session.spawn_child(&root, ChildConfig::default(), None);
    assert_eq!(
        refused,
        Err(SpawnRefused::TooManyChildren { live: 8, max: 8 })
    );
    assert_eq!(session.children_of(&root).len(), 8, "被拒之后子数不该变");
}

#[test]
fn set_agent_limits_does_not_retroactively_kill_existing_children() {
    let mut session = new_session();
    let root = session.agent().clone();

    for _ in 0..8 {
        session
            .spawn_child(&root, ChildConfig::default(), None)
            .expect("先在默认上限内长满");
    }
    assert_eq!(session.children_of(&root).len(), 8);

    session.set_agent_limits(AgentLimits {
        max_depth: 3,
        max_children: 2,
    });
    assert_eq!(
        session.agent_limits(),
        AgentLimits {
            max_depth: 3,
            max_children: 2
        }
    );

    // 已经存在的 8 个子一个都不会被清理。
    let survivors = session.children_of(&root);
    assert_eq!(survivors.len(), 8);
    for child in &survivors {
        assert!(session.is_live(child));
    }

    // 但新的一次 spawn 立刻按新上限拒绝。
    let refused = session.spawn_child(&root, ChildConfig::default(), None);
    assert_eq!(
        refused,
        Err(SpawnRefused::TooManyChildren { live: 8, max: 2 })
    );
}

#[test]
fn lowering_max_depth_does_not_kill_an_existing_deep_agent() {
    let mut session = new_session();
    let a1 = session
        .spawn_child(&session.agent().clone(), ChildConfig::default(), None)
        .expect("depth 1");
    let a2 = session
        .spawn_child(&a1, ChildConfig::default(), None)
        .expect("depth 2");
    let a3 = session
        .spawn_child(&a2, ChildConfig::default(), None)
        .expect("depth 3");
    assert_eq!(a3.depth(), 3);

    session.set_agent_limits(AgentLimits {
        max_depth: 1,
        max_children: 8,
    });

    assert!(
        session.is_live(&a3),
        "已经存在的深度 3 agent 不该被追溯清理"
    );

    let refused = session.spawn_child(&a1, ChildConfig::default(), None);
    assert_eq!(
        refused,
        Err(SpawnRefused::DepthExceeded { depth: 2, max: 1 })
    );
}
