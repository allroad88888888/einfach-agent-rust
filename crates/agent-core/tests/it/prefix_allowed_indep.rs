//! 144 独立测试：`Slot::PrefixAllowed`——spawn 时随快照写入的「开局产物」授予名单。
//!
//! **独立测试声明**：本文件只依据 docs/issues/144-prefix-allowed-slot.md 的
//! 「验收」「注意」两节、docs/INVARIANTS.md 红线 3/4/11，以及
//! `subagent_indep_undo_spawn.rs` / `subagent_indep_snapshot.rs` /
//! `host_skills_indep_restore.rs` 里 `spawn_child` 的现行调用姿势与
//! 快照/恢复/undo 的断言手法写成——未读 `crates/agent-core/src/` 下任何实现
//! 文件。实现在并行落地，本文件在实现落地前编译红是预期状态。
//!
//! 覆盖：spawn 传 `Some`（乱序 + 重复）落盘时排序去重、传 `None` 落「不设限」、
//! `Some(vec![])`（全不带）与 `None`（不设限）是两个不同的值、不能被实现塌成
//! 同一个、快照→恢复（含 serde 字符串逐字节 roundtrip）、undo 撤掉 spawn 之后
//! 槽位回默认（带正控，防止「实现从头到尾都返回 None」这种假过）、root（从未
//! 被 spawn 过）读出 `None`。
//!
//! 同构先例：`Slot::ToolsAllowed`（028）——本槽位是它的照抄体，编解码先例见
//! `SkillsActive`/`DisabledBuiltins` 共用的 `value::str_set`（见 144 文档
//! 「现状」）。134 把每个 agent 的槽位数钉到 19（`subagent_indep_snapshot.rs`
//! 的 `primitives_of_a_two_child_session_cover_the_whole_tree`），144 的
//! `ALL` 数组末尾追加 `PrefixAllowed` 之后应为 20——本文件里 `child_slot_count`
//! 的期望值直接取自 144 文档「做什么」第 1 条，不是读实现猜出来的。

use crate::support::session::new_session;
use agent_core::{AgentId, AtomKey, ChildConfig, DEFAULT_HISTORY_CAP, Session, Slot, UndoReport};

/// 数一个子 agent 名下挂了多少个 primitive 槽位——用来验证 undo 只回滚值、
/// 不逐出 atom（跟 `subagent_indep_undo_spawn.rs` 的同名助手同一手法）。
fn child_slot_count(session: &Session, child: &AgentId) -> usize {
    session
        .primitives()
        .iter()
        .filter(|(k, _)| k.agent() == child)
        .count()
}

/// 144 把每个 agent 的槽位数从 019 定的 19 追加到 20。
const SLOTS_PER_AGENT_AFTER_144: usize = 20;

#[test]
fn spawn_with_some_sorts_and_dedupes_the_granted_names() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec!["b".into(), "a".into(), "a".into()]),
        )
        .expect("spawn child");

    assert_eq!(
        session.prefix_allowed_of(&child),
        Some(vec!["a".into(), "b".into()]),
        "乱序 + 重复的名单落盘时该排序去重（红线 11）"
    );
}

#[test]
fn spawn_with_none_leaves_prefix_allowed_unset() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn child");

    assert_eq!(
        session.prefix_allowed_of(&child),
        None,
        "传 None 该落 Null，读回是「不设限」"
    );
}

/// 「全不带」（`Some(空表)`）与「不设限」（`None`）是两个不同的值——别让编码
/// 把它们塌成一个。
#[test]
fn spawn_with_empty_vec_is_distinct_from_none() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(&root, ChildConfig::default(), Some(vec![]))
        .expect("spawn child");

    assert_eq!(
        session.prefix_allowed_of(&child),
        Some(vec![]),
        "Some(空表) 是「全不带」，不能被读成 None（「不设限」）"
    );
}

#[test]
fn root_that_was_never_spawned_has_no_prefix_allowed() {
    let session = new_session();
    let root = session.agent().clone();
    assert_eq!(
        session.prefix_allowed_of(&root),
        None,
        "root 从未被 spawn 过，没有 spawn 时刻定死的名单"
    );
}

/// 快照 → 公开恢复面 `Session::restore`（照 `subagent_indep_snapshot.rs` 的
/// `restore_from_the_public_surface_rebuilds_the_whole_tree` 手法）→ 值不变。
#[test]
fn prefix_allowed_survives_the_public_restore_surface() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec!["z".into(), "m".into(), "z".into()]),
        )
        .expect("spawn child");

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
        agent_core::AgentLimits::default(),
        &mut |k| unknown_keys.push(k.clone()),
    )
    .expect("恢复不该拒绝一份自己刚生成的落盘件");

    assert!(
        unknown_keys.is_empty(),
        "本版本生成的日志不该出现『不认识的键』：{unknown_keys:?}"
    );
    assert_eq!(
        restored.prefix_allowed_of(&child),
        session.prefix_allowed_of(&child),
        "恢复后 prefix_allowed_of 该和恢复前完全一致"
    );
    assert_eq!(
        restored.prefix_allowed_of(&child),
        Some(vec!["m".into(), "z".into()]),
        "排序去重后的名单也该原样回来，不是恢复前后碰巧相等的两个 None"
    );
}

/// 红线 3 + 红线 11：`PrefixAllowed` 这一条 snapshot entry 单独拎出来 serde
/// 往返，且两次序列化逐字节相同（照 `host_skills_indep_restore.rs` 的
/// `the_declaration_survives_a_serde_roundtrip_byte_for_byte` 手法）。
#[test]
fn prefix_allowed_snapshot_entry_survives_serde_byte_for_byte() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec!["b".into(), "a".into()]),
        )
        .expect("spawn child");

    let snapshot: Vec<(AtomKey, agent_core::AgentValue)> = session
        .primitives()
        .into_iter()
        .filter(|(k, _)| *k == AtomKey::Agent(child.clone(), Slot::PrefixAllowed))
        .collect();
    assert_eq!(snapshot.len(), 1, "PrefixAllowed 槽位该存在且唯一");

    let once = serde_json::to_string(&snapshot).expect("快照该可序列化（红线 3）");
    let back: Vec<(AtomKey, agent_core::AgentValue)> =
        serde_json::from_str(&once).expect("也该能反序列化回来");
    let twice = serde_json::to_string(&back).expect("往返之后仍该可序列化");

    assert_eq!(once, twice, "同一份名单两次序列化必须逐字节相同（红线 11）");
    assert_eq!(snapshot, back, "往返前后值本身也该逐字节相同");
}

/// undo 撤掉 spawn → 该子 agent 的槽位回默认（照 `subagent_indep_undo_spawn.rs`
/// 的断言结构）。**带正控**：undo 之前先证明这个槽位真的被写过、能读出非默认
/// 值——否则一个「实现从头到尾都返回 None」的假实现也会让下面的断言全绿。
#[test]
fn undoing_the_spawn_resets_prefix_allowed_to_none_with_positive_control() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec!["b".into(), "a".into()]),
        )
        .expect("spawn child");

    assert!(session.is_live(&child));

    // 正控：undo 之前，槽位确实带着 spawn 时写入的那份名单，不是默认值。
    assert_eq!(
        session.prefix_allowed_of(&child),
        Some(vec!["a".into(), "b".into()]),
        "undo 之前，prefix_allowed_of 该能读到刚写入的名单（正控，防假过）"
    );
    assert_eq!(
        child_slot_count(&session, &child),
        SLOTS_PER_AGENT_AFTER_144
    );

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "undo_turn 该 Applied，实际 {report:?}"
    );

    assert!(!session.is_live(&child), "撤回 spawn 之后子不该再活着");

    // 裁决的核心（同 028）：atom 还在，槽位数不变，只是值回默认。
    assert_eq!(
        child_slot_count(&session, &child),
        SLOTS_PER_AGENT_AFTER_144,
        "undo 不逐出 atom，只回滚值——这是跟 despawn 墓碑语义的关键区别"
    );
    assert_eq!(
        session.prefix_allowed_of(&child),
        None,
        "undo 之后该回默认（Null → None），不是继续带着 spawn 时的名单"
    );
}
