//! 010 验收 1：全链路往返 —— capture 一份 primitive 快照，在全新 Store 上用同一个
//! 构图函数重建图，restore 回去，所有 primitive 逐值相等，两层 derived（其一依赖
//! 另一 derived）重算后与原世界相等。
//!
//! `build_graph` 是唯一的构图函数：原 store 和新 store 各调用一次，两侧的 AtomId
//! 完全独立分配——这正是要验证的：快照的键是逻辑键（`String`），不依赖任何一侧
//! `AtomId` 的分配顺序（红线 4）。

use std::cell::RefCell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use agent_store::{
    capture, restore, AtomFamily, AtomId, AtomValue, Snapshot, Store,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tv(i64);

impl AtomValue for Tv {
    fn null() -> Self {
        Tv(0)
    }
}

const PRIMITIVE_KEYS: [&str; 3] = ["p1", "p2", "p3"];

struct Graph {
    store: Store<Tv>,
    family: Rc<RefCell<AtomFamily<String>>>,
}

impl Graph {
    fn slot(&self, key: &str) -> AtomId {
        self.family
            .borrow()
            .get(&key.to_string())
            .unwrap_or_else(|| panic!("build_graph must pre-create {key}"))
    }

    fn value(&self, key: &str) -> Tv {
        self.store.get(self.slot(key))
    }
}

/// 3 个 primitive（p1/p2/p3）+ 2 个 derived（d1 = p1+p2，d2 = d1*p3 —— d2 依赖 d1，
/// d1 是另一个 derived）。全部按逻辑键登记进 family。
fn build_graph() -> Graph {
    let store: Store<Tv> = Store::new();
    let family: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));

    let p1 = store.create_atom(Tv(0));
    let p2 = store.create_atom(Tv(0));
    let p3 = store.create_atom(Tv(0));
    family.borrow_mut().attach("p1".to_string(), p1);
    family.borrow_mut().attach("p2".to_string(), p2);
    family.borrow_mut().attach("p3".to_string(), p3);

    let d1 = store.create_derived_ctx(move |args| Tv(args.get(p1).0 + args.get(p2).0));
    family.borrow_mut().attach("d1".to_string(), d1);

    let d2 = store.create_derived_ctx(move |args| Tv(args.get(d1).0 * args.get(p3).0));
    family.borrow_mut().attach("d2".to_string(), d2);

    Graph { store, family }
}

#[test]
fn primitives_and_two_layers_of_derived_match_after_capture_and_restore_on_a_fresh_store() {
    let original = build_graph();

    // 写几轮。
    original.store.set(original.slot("p1"), Tv(1));
    original.store.set(original.slot("p2"), Tv(2));
    original.store.set(original.slot("p3"), Tv(3));
    original.store.set(original.slot("p1"), Tv(10));
    original.store.set(original.slot("p2"), Tv(20));
    original.store.set(original.slot("p3"), Tv(4));

    let expected_d1 = original.value("d1"); // 10 + 20 = 30
    let expected_d2 = original.value("d2"); // 30 * 4 = 120
    assert_eq!(expected_d1, Tv(30));
    assert_eq!(expected_d2, Tv(120));

    // 只存 primitive。
    let atoms = PRIMITIVE_KEYS
        .iter()
        .copied()
        .map(|k: &str| (k.to_string(), original.slot(k)));
    let snap: Snapshot<String, Tv> = capture(&original.store, atoms);
    assert_eq!(snap.values.len(), 3);

    // 全新 Store，同一个构图函数重建图 —— 这一侧的 AtomId 是这个 store 自己独立
    // 分配的（哪怕数值和原 store 撞了也无所谓：AtomId 只在各自的 Store 内有意义，
    // 快照靠的是逻辑键 String，不是这个数值——红线 4）。
    let fresh = build_graph();

    let mut unknown: Vec<String> = Vec::new();
    let mut resolve = |k: &String| fresh.family.borrow().get(k);
    restore(&fresh.store, &mut resolve, &snap, &mut |k: &String| {
        unknown.push(k.clone())
    });

    assert!(unknown.is_empty());
    assert_eq!(fresh.value("p1"), original.value("p1"));
    assert_eq!(fresh.value("p2"), original.value("p2"));
    assert_eq!(fresh.value("p3"), original.value("p3"));

    // 两层 derived 重算后与原世界相等 —— d2 依赖 d1，d1 依赖 primitive。
    assert_eq!(fresh.value("d1"), expected_d1);
    assert_eq!(fresh.value("d2"), expected_d2);
}
