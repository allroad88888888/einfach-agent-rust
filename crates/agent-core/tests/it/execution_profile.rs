//! 093 B 分支：子 agent 的 durable execution identity。

use std::sync::Arc;

use agent_core::{
    AgentId, AgentValue, AtomKey, ChildConfig, DEFAULT_HISTORY_CAP, ExecutionProfileId, Session,
    Slot, UndoReport, Visibility,
};

fn explicit_config(id: &str) -> ChildConfig {
    ChildConfig {
        tools_allowed: vec![Arc::from("builtin:shell")],
        execution_profile: Some(ExecutionProfileId::new(id)),
        ..ChildConfig::default()
    }
}

#[test]
fn default_is_null_and_explicit_profile_shares_the_spawn_entry() {
    let root = AgentId::root();
    let mut default_session = Session::new(root.clone());
    let default_child = default_session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("default child");

    assert_eq!(default_session.execution_profile_of(&root), None);
    assert_eq!(default_session.execution_profile_of(&default_child), None);
    assert_eq!(
        default_session
            .history()
            .entries()
            .last()
            .unwrap()
            .changes
            .len(),
        1,
        "Null → Null 不该制造幽灵 change"
    );

    let mut explicit_session = Session::new(root.clone());
    let profile = ExecutionProfileId::new("worker-low-risk");
    let child = explicit_session
        .spawn_child(&root, explicit_config(profile.as_str()), None)
        .expect("explicit child");

    assert_eq!(
        explicit_session.execution_profile_of(&child),
        Some(profile.clone())
    );
    assert_eq!(Slot::ExecutionProfile.visibility(), Visibility::Private);
    assert_eq!(explicit_session.history_len(), 1);

    let spawn = explicit_session.history().entries().last().unwrap();
    assert_eq!(spawn.meta.label, "spawn_child");
    assert_eq!(
        spawn.changes.len(),
        2,
        "工具授权与 profile 必须同 entry 落盘"
    );
    assert!(
        spawn
            .changes
            .iter()
            .any(|change| { change.key == AtomKey::Agent(child.clone(), Slot::ToolsAllowed) })
    );
    let profile_change = spawn
        .changes
        .iter()
        .find(|change| change.key == AtomKey::Agent(child.clone(), Slot::ExecutionProfile))
        .expect("spawn entry must contain execution profile");
    assert_eq!(profile_change.prev, AgentValue::Null);
    assert_eq!(
        profile_change.next,
        AgentValue::Text(Arc::from("worker-low-risk"))
    );
}

#[test]
fn undo_and_redo_remove_and_restore_the_same_profile() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let profile = ExecutionProfileId::new("worker-replay");
    let child = session
        .spawn_child(&root, explicit_config(profile.as_str()), None)
        .expect("child");

    assert!(matches!(session.undo_turn(), UndoReport::Applied { .. }));
    assert_eq!(session.execution_profile_of(&child), None);
    assert!(!session.is_live(&child));

    assert!(matches!(session.redo_turn(), UndoReport::Applied { .. }));
    assert_eq!(session.execution_profile_of(&child), Some(profile));
    assert!(session.is_live(&child));
}

#[test]
fn child_retry_override_is_atomic_with_the_trusted_profile() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: Vec::new(),
                execution_profile: Some(ExecutionProfileId::new("worker")),
                max_retries: Some(0),
            },
            None,
        )
        .expect("worker child");

    assert!(session.primitives().iter().any(|(key, value)| {
        key == &AtomKey::Agent(child.clone(), Slot::MaxRetries) && value == &AgentValue::U64(0)
    }));
    let spawn = session.history().entries().last().unwrap();
    assert_eq!(spawn.meta.label, "spawn_child");
    assert_eq!(spawn.changes.len(), 3);
    assert!(spawn.changes.iter().any(|change| {
        change.key == AtomKey::Agent(child.clone(), Slot::MaxRetries)
            && change.next == AgentValue::U64(0)
    }));
}

#[test]
fn snapshot_restore_preserves_profile_and_legacy_missing_slot_is_null() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let profile = ExecutionProfileId::new("worker-restored");
    let child = session
        .spawn_child(&root, explicit_config(profile.as_str()), None)
        .expect("child");
    let entries: Vec<_> = session.history().entries().cloned().collect();
    let cursor = session.cursor();
    let next_seq = entries.last().map_or(0, |entry| entry.seq + 1);
    let snapshot = session.primitives();

    let mut replay_unknown = Vec::new();
    let replayed = Session::restore(
        root.clone(),
        None,
        entries,
        cursor,
        next_seq,
        DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |key| replay_unknown.push(key.clone()),
    )
    .expect("restore from spawn log");
    assert!(replay_unknown.is_empty());
    assert_eq!(replayed.execution_profile_of(&child), Some(profile.clone()));
    assert!(replayed.is_live(&child));

    let mut unknown = Vec::new();
    let restored = Session::restore(
        root.clone(),
        Some(snapshot.clone()),
        Vec::new(),
        0,
        0,
        DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |key| unknown.push(key.clone()),
    )
    .expect("restore current snapshot");
    assert!(unknown.is_empty());
    assert_eq!(restored.execution_profile_of(&child), Some(profile));
    assert!(restored.is_live(&child));

    let legacy_snapshot: Vec<_> = snapshot
        .into_iter()
        .filter(|(key, _)| !matches!(key, AtomKey::Agent(_, Slot::ExecutionProfile)))
        .collect();
    let mut legacy_unknown = Vec::new();
    let legacy = Session::restore(
        root,
        Some(legacy_snapshot),
        Vec::new(),
        0,
        0,
        DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |key| legacy_unknown.push(key.clone()),
    )
    .expect("restore legacy snapshot without execution profile slot");

    assert!(legacy_unknown.is_empty());
    assert_eq!(legacy.execution_profile_of(&child), None);
    assert!(legacy.is_live(&child));
}
