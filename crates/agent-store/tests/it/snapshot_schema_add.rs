//! 010 验收 2：schema 演进·新增。新 store 的构图函数比快照时**多一个** primitive
//! （`p3`，插在 p1/p2 中间）→ restore 旧快照 → 新 atom 保持默认值、旧键全恢复、
//! `on_unknown` 没被叫。

use serde::{Deserialize, Serialize};

use agent_store::{AtomFamily, AtomValue, Snapshot, Store, capture, restore};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tv(i64);

impl AtomValue for Tv {
    fn null() -> Self {
        Tv(0)
    }
}

const NEW_ATOM_DEFAULT: Tv = Tv(99);

/// 快照时刻的构图函数：只有 p1、p2。
fn build_old() -> (Store<Tv>, AtomFamily<String>) {
    let store: Store<Tv> = Store::new();
    let mut family: AtomFamily<String> = AtomFamily::new();
    family.get_or_create("p1".to_string(), || store.create_atom(Tv(0)));
    family.get_or_create("p2".to_string(), || store.create_atom(Tv(0)));
    (store, family)
}

/// 恢复时刻的构图函数：中间插了一个新 primitive p3，默认值 99（挑一个既不是 0
/// 也不是任何快照值的数，好确认它没被 restore 碰过）。
fn build_new() -> (Store<Tv>, AtomFamily<String>) {
    let store: Store<Tv> = Store::new();
    let mut family: AtomFamily<String> = AtomFamily::new();
    family.get_or_create("p1".to_string(), || store.create_atom(Tv(0)));
    family.get_or_create("p3".to_string(), || store.create_atom(NEW_ATOM_DEFAULT));
    family.get_or_create("p2".to_string(), || store.create_atom(Tv(0)));
    (store, family)
}

#[test]
fn a_new_atom_keeps_its_default_and_old_keys_restore_with_no_unknowns() {
    let (old_store, old_family) = build_old();
    let p1 = old_family.get(&"p1".to_string()).unwrap();
    let p2 = old_family.get(&"p2".to_string()).unwrap();
    old_store.set(p1, Tv(5));
    old_store.set(p2, Tv(6));

    let atoms = vec![("p1".to_string(), p1), ("p2".to_string(), p2)];
    let snap: Snapshot<String, Tv> = capture(&old_store, atoms.into_iter());

    let (new_store, new_family) = build_new();
    let p3 = new_family.get(&"p3".to_string()).unwrap();
    assert_eq!(new_store.get(p3), NEW_ATOM_DEFAULT); // restore 之前的基线

    let mut unknown: Vec<String> = Vec::new();
    let mut resolve = |k: &String| new_family.get(k);
    restore(&new_store, &mut resolve, &snap, &mut |k: &String| {
        unknown.push(k.clone())
    });

    assert!(
        unknown.is_empty(),
        "旧快照的两个键都认识，不该叫 on_unknown"
    );
    assert_eq!(
        new_store.get(new_family.get(&"p1".to_string()).unwrap()),
        Tv(5)
    );
    assert_eq!(
        new_store.get(new_family.get(&"p2".to_string()).unwrap()),
        Tv(6)
    );
    assert_eq!(new_store.get(p3), NEW_ATOM_DEFAULT); // 新 atom 保持默认值，没被碰过
}
