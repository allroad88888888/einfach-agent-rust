//! 209 独立测试（三）：`Slot::Notes` 站 `Private`——别的 agent（父、子、兄弟）
//! 用 `read_agent` 一律读不到，但自己读自己不受影响；子 agent 与父 agent 的草稿纸
//! 是同一个 store 里两把不同的锁，互不影响。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「验收」（「Private 守住」「子 agent 的
//! notes 与父的互不影响」两条）、docs/INVARIANTS.md 红线 10、以及派我这份任务的
//! 独立测试 agent 说明里给的正门签名
//! `Session::read_agent(&AgentId, Slot) -> Result<AgentValue, ReadDenied>`。
//! **实现体一行没读**（见 `notes_indep_basic.rs` 顶部）。

use std::sync::Arc;

use crate::support::session::new_session;
use agent_core::{AgentId, ChildConfig, ReadDenied, Session, Slot};

fn note(text: &str) -> Arc<str> {
    Arc::from(text)
}

/// root + 两个直接子 agent——够覆盖「父读子」「子读父」「兄弟读兄弟」三个方向
/// （照 `inbox_indep.rs::tree()` 同一个形状，这里不复用那个文件是为了不让
/// notes 专题的夹具依赖 inbox 专题的夹具）。
fn tree() -> (Session, AgentId, AgentId, AgentId) {
    let mut session = new_session();
    let root = session.agent().clone();
    let a1 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a1");
    let a2 = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn a2");
    (session, root, a1, a2)
}

/// 子读父的 `Notes`：拒绝。`read_agent` 本身不带「调用方是谁」的参数
/// （判据只看被读槽位的 `Visibility`，不看谁在读——决策 35 之后横读不限方向），
/// 所以这条断言的是「无论站在哪个 agent 的视角，root 的 `Notes` 都读不到」。
#[test]
fn a_child_cannot_read_the_parents_notes() {
    let (mut session, root, _a1, _a2) = tree();
    session
        .set_note(&root, note("秘密"), Some(note("只有 root 自己看得到")))
        .expect("root 写自己的草稿纸");

    match session.read_agent(&root, Slot::Notes) {
        Err(ReadDenied::NotVisible { .. }) => {}
        other => panic!("子读父的 Notes 该被拒，得到 {other:?}"),
    }
}

/// 父读子的 `Notes`：拒绝——决策 35 横读全开也没有放开 `Private` 这一格。
#[test]
fn a_parent_cannot_read_a_childs_notes() {
    let (mut session, _root, a1, _a2) = tree();
    session
        .set_note(&a1, note("k"), Some(note("a1 自己的东西")))
        .expect("a1 写自己的草稿纸");

    match session.read_agent(&a1, Slot::Notes) {
        Err(ReadDenied::NotVisible { .. }) => {}
        other => panic!("父读子的 Notes 该被拒，得到 {other:?}"),
    }
}

/// 兄弟读兄弟的 `Notes`：同样拒绝，即便兄弟刚往自己的草稿纸上写过东西。
#[test]
fn a_sibling_cannot_read_the_other_siblings_notes() {
    let (mut session, _root, _a1, a2) = tree();
    session
        .set_note(&a2, note("k"), Some(note("a2 自己的东西")))
        .expect("a2 写自己的草稿纸");

    match session.read_agent(&a2, Slot::Notes) {
        Err(ReadDenied::NotVisible { .. }) => {}
        other => panic!("兄弟读兄弟的 Notes 该被拒，得到 {other:?}"),
    }
}

/// `Private` 说的是「**别的** agent 读不到」，不是「自己也读不到」：上面三条
/// 拒绝之后，各自的 agent 用 `notes_of`（自读口）照样看得到自己刚写的东西。
#[test]
fn self_read_still_works_after_cross_agent_reads_are_denied() {
    let (mut session, root, a1, a2) = tree();
    session
        .set_note(&root, note("k"), Some(note("root 的")))
        .unwrap();
    session
        .set_note(&a1, note("k"), Some(note("a1 的")))
        .unwrap();
    session
        .set_note(&a2, note("k"), Some(note("a2 的")))
        .unwrap();

    // 三个方向的跨读全部先确认会被拒（正控：不是因为这个会话压根没写过东西）。
    assert!(matches!(
        session.read_agent(&a1, Slot::Notes),
        Err(ReadDenied::NotVisible { .. })
    ));
    assert!(matches!(
        session.read_agent(&root, Slot::Notes),
        Err(ReadDenied::NotVisible { .. })
    ));

    assert_eq!(session.notes_of(&root).get("k").map(|v| &**v), Some("root 的"));
    assert_eq!(session.notes_of(&a1).get("k").map(|v| &**v), Some("a1 的"));
    assert_eq!(session.notes_of(&a2).get("k").map(|v| &**v), Some("a2 的"));
}

/// **子 agent 的 notes 与父的互不影响**（209 验收原文）：在子上写一堆 key，
/// 父的草稿纸一个字节都不该动；反过来也一样——同一个 store，靠 family 的
/// `AgentId` 区分实例，不是共用一张表。
#[test]
fn a_childs_notes_and_the_parents_notes_do_not_leak_into_each_other() {
    let (mut session, root, a1, _a2) = tree();

    session
        .set_note(&root, note("shared_key_name"), Some(note("root 的版本")))
        .unwrap();
    assert!(session.notes_of(&a1).is_empty(), "写 root 之前 a1 该是空的");

    session
        .set_note(&a1, note("shared_key_name"), Some(note("a1 的版本")))
        .unwrap();
    session
        .set_note(&a1, note("only_on_a1"), Some(note("a1 独有")))
        .unwrap();

    // 即便 key 名字完全相同，两边各自独立——不是同一张表的两个视图。
    assert_eq!(
        session.notes_of(&root).get("shared_key_name").map(|v| &**v),
        Some("root 的版本"),
        "root 的那一份不该被 a1 的写入改掉"
    );
    assert_eq!(
        session.notes_of(&a1).get("shared_key_name").map(|v| &**v),
        Some("a1 的版本")
    );
    assert_eq!(session.notes_of(&root).len(), 1, "root 不该多出 a1 专属的 key");
    assert!(!session.notes_of(&root).contains_key("only_on_a1"));
}
