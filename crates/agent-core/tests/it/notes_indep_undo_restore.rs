//! 209 独立测试（四）：`Slot::Notes` **白拿的那套机制**——`/undo` 连带撤销且不
//! 产生屏障（`Undoability::StateOnly`），落盘往返（`primitives` 序列化 + 从日志
//! 重放）之后条目逐字节回来。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「验收」（「/undo 掉写 notes 的那一轮」
//! 「崩溃恢复」两条）、docs/INVARIANTS.md 红线 3/6/11。**实现体一行没读**
//! （见 `notes_indep_basic.rs` 顶部）。
//!
//! 恢复走的公开面是 `Session::restore`（日志回放）——照既有
//! `inbox_indep_undo_restore.rs::a_snapshot_round_trip_keeps_the_delivery_mark_of_every_item`
//! 的写法：`None` 快照 = 从头重放全部日志。

use std::sync::Arc;

use crate::support::session::new_session;
use crate::support::{provider_done_end_turn, user_input_event};
use agent_core::{
    AgentId, AgentLimits, AgentValue, AtomKey, DEFAULT_HISTORY_CAP, Session, Slot, UndoReport,
    Undoability,
};

/// 跑完一整轮（user_input → 纯文本收尾）再 `begin_turn`，把 `session` 推进到一个
/// **新的 turn**——`undo_turn` 撤的是「当前这个 turn 里的全部 entry」，两次
/// `set_note` 不隔一个 turn 边界的话会被当成同一组一起撤掉（这份测试踩过这个坑：
/// 起初两次写都落在 turn 0，`undo_turn` 把两次写都撤了，回到了空，不是回到 v1）。
/// 照 `session_indep_begin_turn.rs`/`inbox_indep_undo_restore.rs` 同一个手法。
fn advance_to_a_new_turn(session: &mut Session) {
    let _ = session.step(user_input_event("推进到下一轮"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "收尾"));
    session.begin_turn();
}

fn note(text: &str) -> Arc<str> {
    Arc::from(text)
}

/// `/undo` 掉写 notes 的那一轮 → 那条真的没了（读一次 atom 断言，不是看日志说撤
/// 了），而且这一步是 `StateOnly`——没碰外部世界，不该产生屏障。
///
/// **带正控**：undo 之前先证明写入确实生效过，否则一个「写入本来就是空操作」的
/// 假实现也能让下面的「撤销之后是空的」断言全绿。
#[test]
fn undoing_the_note_write_removes_it_with_no_barrier() {
    let mut session = new_session();
    let root = session.agent().clone();
    assert!(session.notes_of(&root).is_empty());

    session
        .set_note(&root, note("k"), Some(note("写进去的东西")))
        .expect("写入该成功");

    // 正控。
    assert_eq!(
        session.notes_of(&root).get("k").map(|v| &**v),
        Some("写进去的东西"),
        "undo 之前先确认写入真的生效了（正控，防假过）"
    );

    let entry = session.history().last().expect("写入该留一条 entry");
    assert_eq!(
        entry.meta.undoability,
        Undoability::StateOnly,
        "notes 的写入不碰外部世界，undo 不该需要任何还原钩子"
    );

    let report = session.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { .. }),
        "StateOnly 不该拦住 undo，得到 {report:?}"
    );

    assert!(
        session.notes_of(&root).is_empty(),
        "撤销之后这条 note 该真的没了——从 notes_of 这个 atom 读口读出来的，不是看\
         日志条目说撤了"
    );
}

/// 覆盖也一样：写 v1 → 写 v2（覆盖）→ undo 最后一轮 → 回到 v1，不是回到空。
/// 这条钉住的是「undo 撤的是最近一次写入」而不是「把整个槽位清空」这两种容易
/// 混淆的实现。
#[test]
fn undoing_an_overwrite_restores_the_previous_value_not_an_empty_slot() {
    let mut session = new_session();
    let root = session.agent().clone();

    session.set_note(&root, note("k"), Some(note("v1"))).unwrap();
    advance_to_a_new_turn(&mut session);
    session.set_note(&root, note("k"), Some(note("v2"))).unwrap();
    assert_eq!(session.notes_of(&root).get("k").map(|v| &**v), Some("v2"));

    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");

    assert_eq!(
        session.notes_of(&root).get("k").map(|v| &**v),
        Some("v1"),
        "撤销的是最后一次覆盖，该回到 v1，不是整槽清空"
    );
}

/// 撤销一次删除（`value = None`）→ 那条 key 该重新出现，值回到删除之前那个。
#[test]
fn undoing_a_deletion_brings_the_key_back() {
    let mut session = new_session();
    let root = session.agent().clone();

    session.set_note(&root, note("k"), Some(note("原始值"))).unwrap();
    advance_to_a_new_turn(&mut session);
    session.set_note(&root, note("k"), None).unwrap();
    assert!(session.notes_of(&root).is_empty(), "正控：确实删掉了");

    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");

    assert_eq!(
        session.notes_of(&root).get("k").map(|v| &**v),
        Some("原始值"),
        "撤销删除该让这条 key 重新出现"
    );
}

/// 找会话 primitives 里 `Slot::Notes` 那一格的当前值。
fn notes_atom_value(session: &Session, agent: &AgentId) -> AgentValue {
    let key = AtomKey::Agent(agent.clone(), Slot::Notes);
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("primitives 里没有 {key:?}"))
}

/// **崩溃恢复**：写几条 → 落盘（`primitives` 序列化，红线 3）→ 从日志重放
/// （`Session::restore`，公开的恢复面）→ 条目逐字节回来，而且恢复出来的会话
/// 继续可写、可读、行为跟原会话一致。
#[test]
fn a_log_replay_round_trip_keeps_every_note_byte_identical() {
    let mut session = new_session();
    let root = session.agent().clone();

    for (k, v) in [("c", "third"), ("a", "first"), ("b", "second")] {
        session.set_note(&root, note(k), Some(note(v))).unwrap();
    }
    let before_notes = session.notes_of(&root);
    let before_value = notes_atom_value(&session, &root);

    // 红线 3：primitive 必须可序列化，且往返之后逐字节相同。
    let snap = session.primitives();
    let json = serde_json::to_string(&snap).expect("primitives 该能序列化（红线 3）");
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&json).expect("也该能读回来");
    assert_eq!(back, snap);
    assert_eq!(
        serde_json::to_string(&back).unwrap(),
        json,
        "序列化往返之后逐字节相同"
    );

    // 公开的恢复面：日志回放（`None` 快照 = 从头重放全部日志）。
    let entries: Vec<_> = session.history().entries().cloned().collect();
    let cursor = session.cursor();
    let next_seq = entries.last().map(|e| e.seq + 1).unwrap_or(0);
    let restored = Session::restore(
        root.clone(),
        None,
        entries,
        cursor,
        next_seq,
        DEFAULT_HISTORY_CAP,
        AgentLimits::default(),
        &mut |k| panic!("恢复遇到不认识的键 {k:?}"),
    )
    .expect("恢复不该拒绝一份自己刚生成的落盘件");

    assert_eq!(
        restored.notes_of(&root),
        before_notes,
        "恢复之后 notes_of 逐条相同——key、value 都算在内"
    );
    assert_eq!(
        notes_atom_value(&restored, &root),
        before_value,
        "恢复之后落盘那份 AgentValue 逐字节相同"
    );
    assert_eq!(restored.primitives(), session.primitives());
}
