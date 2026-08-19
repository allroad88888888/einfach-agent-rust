//! 209 独立测试（一）：`Slot::Notes` 的**基本读写行为**——写一条读得回来、同 key
//! 第二次覆盖、写 `None` 删掉，以及红线 11（序列化顺序按 key 升序、且逐字节确定）。
//!
//! 黑盒来源：docs/issues/209-notes-slot.md「做什么」1/2 与「验收」、
//! docs/INVARIANTS.md 红线 2（写走 command 层）/ 3（primitive 可序列化）/
//! 11（进 prompt 的东西逐字节确定）、以及派我这份任务的独立测试 agent 说明里给的
//! 公开签名（`Session::notes_of` / `Session::set_note` / `NoteDenied`）。
//!
//! **实现体一行没读**：`command/notes.rs`、`value/notes.rs`、`notes_tool.rs`、
//! `notes_render.rs` 及各自的 `_tests.rs` 全程没打开——只看 `graph/slot.rs`、
//! `graph/slot_default.rs`、`graph/visibility.rs`（Slot 名册/默认值/可见性站队，
//! 这三个文件不在禁读名单里）与既有测试的写法（`inbox_indep*.rs`）。

use std::sync::Arc;

use crate::support::session::new_session;
use agent_core::{AgentId, AgentValue, AtomKey, Session, Slot};

/// 只有 root 一个 agent 就够——这一份测的是单 agent 内部的读写行为，
/// 跨 agent 隔离另有 `notes_indep_isolation.rs`。
fn root_session() -> (Session, AgentId) {
    let session = new_session();
    let root = session.agent().clone();
    (session, root)
}

fn note(text: &str) -> Arc<str> {
    Arc::from(text)
}

/// 写一条 → 读回来一模一样。
#[test]
fn writing_a_note_then_reading_it_back_is_exact() {
    let (mut session, root) = root_session();
    assert!(session.notes_of(&root).is_empty(), "新会话的草稿纸该是空的");

    session
        .set_note(&root, note("todo"), Some(note("买牛奶")))
        .expect("正常写入不该被拒");

    let notes = session.notes_of(&root);
    assert_eq!(notes.len(), 1);
    assert_eq!(notes.get("todo").map(|v| &**v), Some("买牛奶"));
}

/// 写同一个 key 第二次 → 覆盖，不是追加成两条。
#[test]
fn writing_the_same_key_twice_overwrites_not_appends() {
    let (mut session, root) = root_session();

    session
        .set_note(&root, note("todo"), Some(note("买牛奶")))
        .expect("第一次写入");
    session
        .set_note(&root, note("todo"), Some(note("买鸡蛋")))
        .expect("第二次写入（覆盖）");

    let notes = session.notes_of(&root);
    assert_eq!(notes.len(), 1, "同一个 key 该只留一条，不是两条");
    assert_eq!(notes.get("todo").map(|v| &**v), Some("买鸡蛋"));
}

/// 写 `None` / 空 → 删掉这条。
#[test]
fn writing_none_deletes_the_key() {
    let (mut session, root) = root_session();
    session
        .set_note(&root, note("todo"), Some(note("买牛奶")))
        .expect("先写一条");
    assert_eq!(session.notes_of(&root).len(), 1, "正控：确实写进去了");

    session
        .set_note(&root, note("todo"), None)
        .expect("value=None 该被当作删除接受，不是拒绝");

    assert!(
        session.notes_of(&root).is_empty(),
        "删掉之后草稿纸该重新变空"
    );
}

/// 删一个从没写过的 key：不该报错（幂等的「反正现在没有这条」），也不该凭空
/// 生出一条空条目。
#[test]
fn deleting_a_key_that_was_never_written_is_harmless() {
    let (mut session, root) = root_session();
    let result = session.set_note(&root, note("从没写过"), None);
    assert!(result.is_ok(), "删一条本来就不存在的 key 不该被拒：{result:?}");
    assert!(session.notes_of(&root).is_empty());
}

/// 两个不同的 key 各自独立存在，互不覆盖。
#[test]
fn two_different_keys_coexist() {
    let (mut session, root) = root_session();
    session
        .set_note(&root, note("a"), Some(note("第一条")))
        .expect("写 a");
    session
        .set_note(&root, note("b"), Some(note("第二条")))
        .expect("写 b");

    let notes = session.notes_of(&root);
    assert_eq!(notes.len(), 2);
    assert_eq!(notes.get("a").map(|v| &**v), Some("第一条"));
    assert_eq!(notes.get("b").map(|v| &**v), Some("第二条"));
}

/// 找会话 primitives 里 `Slot::Notes` 那一个键，返回它当前的 `AgentValue`。
/// 找不到就直接 panic——这条键必须存在（`Slot::ALL` 名册保证每个 agent 都建）。
fn notes_atom_value(session: &Session, agent: &AgentId) -> AgentValue {
    let key = AtomKey::Agent(agent.clone(), Slot::Notes);
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v)
        .unwrap_or_else(|| panic!("primitives 里没有 {key:?}——Slot::ALL 名册漏了它？"))
}

/// **红线 11 的核心**：三条 key 乱序写入（c、a、b），落盘那份 `AgentValue::Json`
/// 里的数组顺序是 key **升序**（a、b、c），不是写入顺序（c、a、b）。
///
/// 这条挂的是「进 prompt 的东西顺序不定 = 每轮都重排、前缀缓存全价」那类静默代价
/// ——`Notes` 用 `BTreeMap` 存在内存里天然有序，这条测的是**落盘那份序列化**是否
/// 真的把这份有序性带出来，而不是内存里有序、编码时又乱了。
#[test]
fn three_keys_written_out_of_order_serialize_key_ascending() {
    let (mut session, root) = root_session();

    for (k, v) in [("c", "third"), ("a", "first"), ("b", "second")] {
        session
            .set_note(&root, note(k), Some(note(v)))
            .unwrap_or_else(|e| panic!("写 {k} 该成功：{e:?}"));
    }

    let value = notes_atom_value(&session, &root);
    let AgentValue::Json(json) = value else {
        panic!("Notes 槽位该编码成 AgentValue::Json，拿到 {value:?}");
    };
    let array = json
        .as_array()
        .unwrap_or_else(|| panic!("Notes 该编码成一个数组：{json}"));
    assert_eq!(array.len(), 3);

    let keys: Vec<&str> = array
        .iter()
        .map(|entry| {
            entry
                .as_array()
                .and_then(|pair| pair.first())
                .and_then(|k| k.as_str())
                .unwrap_or_else(|| panic!("每一项该是 [key, value] 对：{entry}"))
        })
        .collect();
    assert_eq!(
        keys,
        vec!["a", "b", "c"],
        "写入顺序是 c,a,b，落盘顺序该是 key 升序 a,b,c：{json}"
    );
}

/// 红线 11 的另一半：**同样的写入序列跑两遍**，落盘那份 JSON 逐字节相同——
/// 没有时间戳、没有随机 id、没有依赖 `HashMap` 迭代顺序这类不确定性。
#[test]
fn the_same_write_sequence_serializes_byte_identical_across_two_runs() {
    fn run() -> String {
        let (mut session, root) = root_session();
        for (k, v) in [("c", "third"), ("a", "first"), ("b", "second")] {
            session.set_note(&root, note(k), Some(note(v))).unwrap();
        }
        let AgentValue::Json(json) = notes_atom_value(&session, &root) else {
            panic!("该是 AgentValue::Json");
        };
        serde_json::to_string(&*json).unwrap()
    }

    let first = run();
    assert!(!first.is_empty());
    assert_eq!(
        first,
        run(),
        "红线 11：两次跑出来的落盘字节必须逐字节相同"
    );
}

/// 新建的 Notes atom（从没写过）在 `primitives()` 里的原始编码，必须跟「写了
/// 一条又删掉」之后的空草稿纸**逐字节相同**——`graph/slot_default.rs` 模块文档
/// 明说的设计：「读取点不必区分『没写过』和『记了又都删了』，它们就是同一个值」。
///
/// **故意不经 `notes_of`**：那条读口有可能对非预期形状做宽松兜底（比如把
/// `AgentValue::Null` 也读成一张空表），从而掩盖掉 `default_value` 选错的问题
/// ——直接比 `primitives()` 里的原始 `AgentValue` 才是这条不变量真正的落点。
#[test]
fn a_fresh_notes_atom_matches_the_empty_after_write_and_delete_encoding() {
    let (mut session, root) = root_session();
    let pristine = notes_atom_value(&session, &root);

    session.set_note(&root, note("k"), Some(note("v"))).unwrap();
    session.set_note(&root, note("k"), None).unwrap();
    let emptied_after_use = notes_atom_value(&session, &root);

    assert_eq!(
        pristine, emptied_after_use,
        "新建的 atom 跟「写过又删空」的 atom 必须是同一个值——default_value 选错\
         （比如选成 Null）会让这条在这里露出来，即便 notes_of 表面上看都是空的"
    );
}
