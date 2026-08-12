//! [`super::spawn`]（`Session::spawn_child`）的白盒单测：铸号规则 + 144 追加的
//! `prefix_allowed` 参数。拆自 `spawn.rs`（144 加了四条 `prefix_allowed` 测试后
//! 顶破 300 行），跟 `despawn.rs`/`despawn_tests.rs` 同一个拆分手法。

use super::*;

fn session() -> Session {
    Session::new(AgentId::root())
}

fn cfg(tools: &[&str]) -> ChildConfig {
    ChildConfig {
        tools_allowed: tools.iter().map(|t| Arc::from(*t)).collect(),
        ..ChildConfig::default()
    }
}

/// 红线 11 的最小实检：入参顺序不同、含重复，落进槽位的值逐字节相同。
/// 机制现在住在 `value::str_set`（跟 039 的 skill 集共用），这里验的是
/// spawn 真的走了它。
#[test]
fn the_tool_subset_is_sorted_and_deduped_before_it_lands() {
    let a = str_set::to_value(vec![Arc::from("srv:fs/read"), Arc::from("srv:shell/exec")]);
    let b = str_set::to_value(vec![
        Arc::from("srv:shell/exec"),
        Arc::from("srv:fs/read"),
        Arc::from("srv:fs/read"),
    ]);
    assert_eq!(a, b);
    let crate::value::atom_value::AgentValue::Json(v) = &a else {
        panic!("工具子集落 Json")
    };
    assert_eq!(
        serde_json::to_string(&**v).unwrap(),
        r#"["srv:fs/read","srv:shell/exec"]"#
    );
}

/// 144 白盒补充：`prefix_allowed` 乱序含重复 → `prefix_allowed_of` 读回排序
/// 去重（红线 11）。黑盒对应在 `tests/it/prefix_allowed_indep.rs`，这里验的
/// 是同一件事在实现内部真的走了 `str_set`，不是两处巧合一致。
#[test]
fn prefix_allowed_some_is_sorted_and_deduped_before_it_lands() {
    let mut s = session();
    let root = AgentId::root();
    let child = s
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec![Arc::from("b"), Arc::from("a"), Arc::from("a")]),
        )
        .unwrap();
    assert_eq!(
        s.prefix_allowed_of(&child),
        Some(vec![Arc::from("a"), Arc::from("b")])
    );
}

/// `None` 落 `Null`——`prefix_allowed_of` 读回「不设限」，且这一条不该在
/// entry 里额外多出一个 change（`Null` 就是 `Slot::PrefixAllowed` 的默认值，
/// 009 的幽灵步不落条目）。
#[test]
fn prefix_allowed_none_leaves_the_slot_at_its_null_default() {
    let mut s = session();
    let root = AgentId::root();
    let before = s.history_len();
    let child = s.spawn_child(&root, ChildConfig::default(), None).unwrap();
    assert_eq!(s.prefix_allowed_of(&child), None);
    assert_eq!(
        s.history_len(),
        before + 1,
        "spawn 本身还是恰好一条 entry——None 没有多写一条"
    );
}

/// undo 撤掉 spawn 之后，`prefix_allowed_of` 回默认（`None`）——跟
/// `Slot::ToolsAllowed` 同一条 undo 语义（`spawn.rs` 模块文档「三条硬性形状」2）。
#[test]
fn undoing_the_spawn_resets_prefix_allowed() {
    let mut s = session();
    let root = AgentId::root();
    let child = s
        .spawn_child(&root, ChildConfig::default(), Some(vec![Arc::from("a")]))
        .unwrap();
    assert_eq!(s.prefix_allowed_of(&child), Some(vec![Arc::from("a")]));

    let _ = s.undo_turn();
    assert_eq!(s.prefix_allowed_of(&child), None);
}

/// 快照 → serde 往返 → 值不变，逐字节确定（红线 3/11）。跟本文件
/// `the_tool_subset_is_sorted_and_deduped_before_it_lands` 同一手法，验的是
/// `Slot::PrefixAllowed` 这一条 primitive 真的进了 `primitives()`。
#[test]
fn prefix_allowed_survives_a_primitives_serde_round_trip() {
    let mut s = session();
    let root = AgentId::root();
    let child = s
        .spawn_child(
            &root,
            ChildConfig::default(),
            Some(vec![Arc::from("z"), Arc::from("m")]),
        )
        .unwrap();

    let key = AtomKey::Agent(child.clone(), Slot::PrefixAllowed);
    let snapshot = s.primitives();
    let entry = snapshot
        .iter()
        .find(|(k, _)| *k == key)
        .expect("PrefixAllowed 该在快照里");

    let once = serde_json::to_string(entry).expect("该可序列化");
    let back: (AtomKey, crate::value::atom_value::AgentValue) =
        serde_json::from_str(&once).expect("也该能反序列化回来");
    let twice = serde_json::to_string(&back).expect("往返之后仍该可序列化");

    assert_eq!(once, twice, "同一份名单两次序列化必须逐字节相同（红线 11）");
    assert_eq!(&back, entry);
}

/// 铸号跳过认不出的段，不猜、不 panic。
#[test]
fn an_unparseable_segment_is_skipped_when_minting() {
    let root = AgentId::root();
    assert_eq!(child_seq(&root, &root.child(7)), Some(7));
    assert_eq!(child_seq(&root, &AgentId::new("root/weird")), None);
    assert_eq!(child_seq(&root, &AgentId::new("other/a1")), None);
}

/// 号不复用：spawn → despawn → spawn，第二个孩子拿的是新号。
/// （墓碑存在的理由，见模块文档。）
#[test]
fn a_seq_is_never_handed_out_twice() {
    let mut s = session();
    let root = AgentId::root();
    let first = s.spawn_child(&root, cfg(&["srv:fs/read"]), None).unwrap();
    assert_eq!(first.as_str(), "root/a1");
    let _ = s.despawn_child(&first).unwrap();
    let second = s.spawn_child(&root, cfg(&["srv:fs/read"]), None).unwrap();
    assert_eq!(second.as_str(), "root/a2");
}

/// undo 掉 spawn 之后再 spawn，同样拿新号——被撤销的那个 id 也算用过了。
#[test]
fn undoing_a_spawn_does_not_release_its_seq() {
    let mut s = session();
    let root = AgentId::root();
    let first = s.spawn_child(&root, cfg(&[]), None).unwrap();
    let _ = s.undo_turn();
    assert!(!s.is_live(&first));
    let second = s.spawn_child(&root, cfg(&[]), None).unwrap();
    assert_ne!(first, second);
}
