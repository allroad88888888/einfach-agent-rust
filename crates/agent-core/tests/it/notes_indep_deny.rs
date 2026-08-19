//! 209 独立测试（二）：`set_note` 的**拒绝面**——空 key、key 超长、value 超长、
//! 条目数撞顶，每一种都是显式的 `NoteDenied` 变体，不是静默截断或静默丢弃。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「做什么」2 与「验收」（「超上限」一条：
//! 「单条超长 → 截断并如实说；条目数撞顶 → 显式拒，不静默丢」——**这一条的截断说的
//! 是工具层**，`Session::set_note` 是 core 层，这里断言的是 core 层直接拒绝，
//! 两层行为不同的另一半在 `agent-runtime` 的 `notes_indep_truncation.rs`）、
//! 公开签名给的 `NoteDenied` 六个变体。**实现体一行没读**（见 `notes_indep_basic.rs`
//! 顶部）。

use std::sync::Arc;

use crate::support::session::new_session;
use agent_core::{AgentId, MAX_NOTES, NOTE_KEY_CAP, NOTE_VALUE_CAP, NoteDenied, Session};

fn root_session() -> (Session, AgentId) {
    let session = new_session();
    let root = session.agent().clone();
    (session, root)
}

fn note(text: &str) -> Arc<str> {
    Arc::from(text)
}

/// 空 key：拒绝，不落任何条目。
#[test]
fn an_empty_key_is_refused() {
    let (mut session, root) = root_session();

    let result = session.set_note(&root, note(""), Some(note("随便什么正文")));
    assert_eq!(result, Err(NoteDenied::EmptyKey));
    assert!(session.notes_of(&root).is_empty(), "被拒的写入不该留下条目");
}

/// key 超过上限：**拒绝，不截断**（跟 value 那条刻意不同——key 是查找的凭据，
/// 截断会让两个原本不同的 key 撞成同一个，是比「变短」更糟的错误）。
#[test]
fn a_key_over_the_cap_is_refused_not_truncated() {
    let (mut session, root) = root_session();
    let long_key: String = "k".repeat(NOTE_KEY_CAP + 1);

    let result = session.set_note(&root, note(&long_key), Some(note("正文")));
    match result {
        Err(NoteDenied::KeyTooLong { bytes, max }) => {
            assert_eq!(bytes, long_key.len());
            assert_eq!(max, NOTE_KEY_CAP);
        }
        other => panic!("超长 key 该拒成 KeyTooLong，得到 {other:?}"),
    }
    assert!(
        session.notes_of(&root).is_empty(),
        "拒绝了就不该有任何条目——包括一条被截断过的"
    );
}

/// 恰好等于上限的 key：合法，不该被拒。
#[test]
fn a_key_exactly_at_the_cap_is_accepted() {
    let (mut session, root) = root_session();
    let key: String = "k".repeat(NOTE_KEY_CAP);

    let result = session.set_note(&root, note(&key), Some(note("正文")));
    assert!(result.is_ok(), "恰好等于上限该被接受，不是拒绝：{result:?}");
    assert_eq!(session.notes_of(&root).len(), 1);
}

/// value 超过上限：**core 层直接拒绝**（不是截断——截断是工具层的事，见模块文档）。
#[test]
fn a_value_over_the_cap_is_refused_at_the_core_layer() {
    let (mut session, root) = root_session();
    let long_value: String = "v".repeat(NOTE_VALUE_CAP + 1);

    let result = session.set_note(&root, note("k"), Some(note(&long_value)));
    match result {
        Err(NoteDenied::ValueTooLong { bytes, max }) => {
            assert_eq!(bytes, long_value.len());
            assert_eq!(max, NOTE_VALUE_CAP);
        }
        other => panic!("超长 value 该拒成 ValueTooLong，得到 {other:?}"),
    }
    assert!(
        session.notes_of(&root).is_empty(),
        "拒绝了就不该有任何条目——包括一条被截断过的"
    );
}

/// 条目数撞顶：写满 `MAX_NOTES` 条不同的 key 都该成功；再写第 `MAX_NOTES + 1`
/// 条**不同的** key 该被**显式拒绝**（不是静默丢弃这一条、也不是静默挤掉最老的
/// 一条）。
#[test]
fn hitting_the_entry_cap_is_refused_explicitly_not_silently_dropped() {
    let (mut session, root) = root_session();

    for i in 0..MAX_NOTES {
        let key = format!("k{i}");
        session
            .set_note(&root, note(&key), Some(note("v")))
            .unwrap_or_else(|e| panic!("第 {i} 条（未撞顶）该成功：{e:?}"));
    }
    assert_eq!(session.notes_of(&root).len(), MAX_NOTES, "该恰好写满上限条");

    let result = session.set_note(&root, note("one_more_new_key"), Some(note("v")));
    match result {
        Err(NoteDenied::TooManyNotes { live, max }) => {
            assert_eq!(live, MAX_NOTES);
            assert_eq!(max, MAX_NOTES);
        }
        other => panic!("撞顶之后新增 key 该显式拒成 TooManyNotes，得到 {other:?}"),
    }
    assert_eq!(
        session.notes_of(&root).len(),
        MAX_NOTES,
        "被拒的写入不该悄悄把条目数推过上限，也不该挤掉已有的任何一条"
    );
}

/// 撞顶之后：**覆盖一条已有的 key 仍然成功**——上限挡的是「新增条目」，不是
/// 「这个 agent 还能不能改自己的草稿纸」。
#[test]
fn overwriting_an_existing_key_still_succeeds_after_the_cap_is_hit() {
    let (mut session, root) = root_session();
    for i in 0..MAX_NOTES {
        session
            .set_note(&root, note(&format!("k{i}")), Some(note("原始值")))
            .expect("填满上限");
    }

    let result = session.set_note(&root, note("k0"), Some(note("改过的值")));
    assert!(
        result.is_ok(),
        "撞顶之后覆盖一条已有的 key 该成功，不是被 TooManyNotes 挡住：{result:?}"
    );

    let notes = session.notes_of(&root);
    assert_eq!(notes.len(), MAX_NOTES, "条目数不该因为一次覆盖而改变");
    assert_eq!(notes.get("k0").map(|v| &**v), Some("改过的值"));
}

/// 撞顶之后：**删掉一条已有的 key 也该成功**，且腾出来的名额能被新 key 占用
/// ——上限守的是「同时存在多少条」，不是「历史上写过多少次」。
#[test]
fn deleting_then_adding_a_new_key_works_after_the_cap_is_hit() {
    let (mut session, root) = root_session();
    for i in 0..MAX_NOTES {
        session
            .set_note(&root, note(&format!("k{i}")), Some(note("v")))
            .expect("填满上限");
    }

    session
        .set_note(&root, note("k0"), None)
        .expect("撞顶之后删除该成功");
    assert_eq!(session.notes_of(&root).len(), MAX_NOTES - 1);

    session
        .set_note(&root, note("brand_new_key"), Some(note("v")))
        .expect("腾出名额之后新增该成功");
    assert_eq!(session.notes_of(&root).len(), MAX_NOTES);
}
