//! 010 验收 3：schema 演进·删除。快照里有键 `ghost`，新图没有它 → resolve 返回
//! `None` → `on_unknown` 恰好收到 `ghost` 一次，其余键照常恢复。

use serde::{Deserialize, Serialize};

use einfach_store::{AtomFamily, AtomValue, Snapshot, Store, capture, restore};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tv(i64);

impl AtomValue for Tv {
    fn null() -> Self {
        Tv(0)
    }
}

/// 快照时刻：p1、p2、ghost 三个 primitive。
fn build_old() -> (Store<Tv>, AtomFamily<String>) {
    let store: Store<Tv> = Store::new();
    let mut family: AtomFamily<String> = AtomFamily::new();
    for key in ["p1", "p2", "ghost"] {
        let st = store.clone();
        family.get_or_create(key.to_string(), || st.create_atom(Tv(0)));
    }
    (store, family)
}

/// 恢复时刻：ghost 被删掉了，构图函数根本不建它 —— `family.get("ghost")` 天然是
/// `None`，不需要任何特判。
fn build_new() -> (Store<Tv>, AtomFamily<String>) {
    let store: Store<Tv> = Store::new();
    let mut family: AtomFamily<String> = AtomFamily::new();
    for key in ["p1", "p2"] {
        let st = store.clone();
        family.get_or_create(key.to_string(), || st.create_atom(Tv(0)));
    }
    (store, family)
}

#[test]
fn a_removed_key_reports_on_unknown_exactly_once_and_the_rest_restores() {
    let (old_store, old_family) = build_old();
    let p1 = old_family.get(&"p1".to_string()).unwrap();
    let p2 = old_family.get(&"p2".to_string()).unwrap();
    let ghost = old_family.get(&"ghost".to_string()).unwrap();
    old_store.set(p1, Tv(1));
    old_store.set(p2, Tv(2));
    old_store.set(ghost, Tv(3));

    let atoms = vec![
        ("p1".to_string(), p1),
        ("p2".to_string(), p2),
        ("ghost".to_string(), ghost),
    ];
    let snap: Snapshot<String, Tv> = capture(&old_store, atoms.into_iter());
    assert_eq!(snap.values.len(), 3);

    let (new_store, new_family) = build_new();
    let mut unknown: Vec<String> = Vec::new();
    let mut resolve = |k: &String| new_family.get(k);
    restore(&new_store, &mut resolve, &snap, &mut |k: &String| {
        unknown.push(k.clone())
    });

    assert_eq!(unknown, vec!["ghost".to_string()]);
    assert_eq!(
        new_store.get(new_family.get(&"p1".to_string()).unwrap()),
        Tv(1)
    );
    assert_eq!(
        new_store.get(new_family.get(&"p2".to_string()).unwrap()),
        Tv(2)
    );
}
