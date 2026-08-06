//! 028 独立测试：多 agent 快照/恢复——`primitives()` 含全树、serde 往返、
//! 公开的 `Session::restore` 面恢复出完整的 `live_agents`/子状态。
//!
//! 黑盒来源：docs/issues/028-multi-agent-graph.md 验收「深度 4/子数 9」邻近的
//! 快照条款、docs/STATE-MODEL.md §「原子图」与 §「恢复 = redo」、cargo doc 的
//! `Session::primitives`/`Session::restore` 文档。`Session::restore` 是公开
//! API，所以直接测它，不必退到 `to_parts`/`from_parts` 那一层。

use crate::support::session::new_session;
use crate::support::user_input_for;
use agent_core::{AgentId, AgentValue, AtomKey, ChildConfig, DEFAULT_HISTORY_CAP, Session};

#[test]
fn primitives_of_a_two_child_session_cover_the_whole_tree() {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
        )
        .expect("a1");
    let a2 = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:web/fetch".into()],
                ..ChildConfig::default()
            },
        )
        .expect("a2");
    session.step(user_input_for(&a1, "a1 说话"));
    session.step(user_input_for(&a2, "a2 说话"));

    let snap = session.primitives();
    let agents_present: std::collections::BTreeSet<&AgentId> =
        snap.iter().map(|(k, _)| k.agent()).collect();

    assert!(agents_present.contains(&root));
    assert!(agents_present.contains(&a1));
    assert!(agents_present.contains(&a2));
    assert_eq!(
        snap.len(),
        45,
        "root + a1 + a2，每个 agent 十五个槽位（093 追加了 ExecutionProfile）"
    );
}

#[test]
fn primitives_survive_a_serde_round_trip_unchanged() {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
        )
        .expect("a1");
    session.step(user_input_for(&a1, "hello"));

    let snap = session.primitives();
    let json = serde_json::to_string(&snap).expect("primitives 该能序列化（红线 3）");
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&json).expect("也该能反序列化回来");

    assert_eq!(snap, back);
}

/// `Session::restore` 是公开面：`None` 快照 + 全量 entries 就是「这个会话从没
/// 落过快照」那条路径（STATE-MODEL：`load()` 直接从头重放全部日志），恢复 =
/// redo 的循环。
#[test]
fn restore_from_the_public_surface_rebuilds_the_whole_tree() {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
        )
        .expect("a1");
    let a2 = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:web/fetch".into()],
                ..ChildConfig::default()
            },
        )
        .expect("a2");
    session.step(user_input_for(&a1, "a1 说话"));
    session.step(user_input_for(&a2, "a2 说话"));

    let entries: Vec<_> = session.history().entries().cloned().collect();
    let cursor = session.cursor();
    let next_seq = entries.last().map(|e| e.seq + 1).unwrap_or(0);

    let mut unknown_keys = Vec::new();
    let restored = Session::restore(
        root.clone(),
        None,
        entries,
        cursor,
        next_seq,
        DEFAULT_HISTORY_CAP,
        &mut |k| unknown_keys.push(k.clone()),
    )
    .expect("恢复不该拒绝一份自己刚生成的落盘件");

    assert!(
        unknown_keys.is_empty(),
        "本版本生成的日志不该出现『不认识的键』"
    );
    assert_eq!(
        restored.live_agents(),
        session.live_agents(),
        "恢复后活着的 agent 集合要完整"
    );
    assert_eq!(restored.children_of(&root), session.children_of(&root));
    assert_eq!(
        restored.primitives(),
        session.primitives(),
        "恢复后每个 agent 的每个槽位值都要完整回来"
    );
}
