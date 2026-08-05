//! 010 验收 4：恢复 == redo 的字面钉。capture 于第 2 条 entry 之后；原 store
//! 继续写第 3、4 条（`record_set` 记录进同一份 `History`）；新 store 上先
//! `restore` 快照，再用 **`apply_next`**（undo/redo 共用的同一个 applier，从
//! `agent_store` 顶层导入，不是本文件另写的重放逻辑）把第 3、4 条重放上去 ——
//! 结果与原世界逐值相等。
//!
//! 这就是 `docs/STATE-MODEL.md` §「恢复 = redo」的字面意思：载入快照 + 把之后的
//! entry 按 next 推一遍，用的是同一个 `apply_next`，不是第二套加载逻辑。

use std::cell::RefCell;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use agent_store::{
    apply_next, capture, record_set, restore, AtomFamily, AtomId, AtomValue, History, Snapshot,
    Store,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Tv(i64);

impl AtomValue for Tv {
    fn null() -> Self {
        Tv(0)
    }
}

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

/// 2 个 primitive（p1/p2）+ 1 个依赖它们的 derived（d）。原 store 与新 store 各
/// 调用一次，各自的 AtomId 互不相干 —— 快照与日志靠 String 键接起两侧。
fn build_graph() -> Graph {
    let store: Store<Tv> = Store::new();
    let family: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));

    let p1 = store.create_atom(Tv(0));
    let p2 = store.create_atom(Tv(0));
    family.borrow_mut().attach("p1".to_string(), p1);
    family.borrow_mut().attach("p2".to_string(), p2);

    let d = store.create_derived_ctx(move |args| Tv(args.get(p1).0 + args.get(p2).0));
    family.borrow_mut().attach("d".to_string(), d);

    Graph { store, family }
}

fn write(
    g: &Graph,
    history: &mut History<String, Tv, String>,
    meta: &str,
    writes: &[(&str, Tv)],
) {
    let mut changes = Vec::new();
    g.store.batch(|s| {
        for (key, next) in writes {
            let id = g.slot(key);
            changes.extend(record_set(s, (*key).to_string(), id, next.clone()));
        }
    });
    history.append(meta.to_string(), changes);
}

#[test]
fn snapshot_plus_apply_next_replay_matches_the_original_world() {
    let original = build_graph();
    let mut history: History<String, Tv, String> = History::new();

    write(&original, &mut history, "e0", &[("p1", Tv(1)), ("p2", Tv(2))]); // seq 0
    write(&original, &mut history, "e1", &[("p1", Tv(3)), ("p2", Tv(4))]); // seq 1 —— 第 2 条

    // capture 于第 2 条 entry 之后。
    let atoms = vec![
        ("p1".to_string(), original.slot("p1")),
        ("p2".to_string(), original.slot("p2")),
    ];
    let snap: Snapshot<String, Tv> = capture(&original.store, atoms.into_iter());

    // 原 store 继续写第 3、4 条 —— record_set 照常记录进同一份 History。
    write(&original, &mut history, "e2", &[("p1", Tv(5)), ("p2", Tv(6))]); // seq 2
    write(&original, &mut history, "e3", &[("p1", Tv(7)), ("p2", Tv(8))]); // seq 3
    assert_eq!(original.value("p1"), Tv(7));
    assert_eq!(original.value("p2"), Tv(8));
    let expected_d = original.value("d");
    assert_eq!(expected_d, Tv(15));

    // 新 store：restore 快照（灌回 capture 时刻的值）。
    let fresh = build_graph();
    let mut unknown: Vec<String> = Vec::new();
    let mut resolve_opt = |k: &String| fresh.family.borrow().get(k);
    restore(&fresh.store, &mut resolve_opt, &snap, &mut |k: &String| {
        unknown.push(k.clone())
    });
    assert!(unknown.is_empty());
    assert_eq!(fresh.value("p1"), Tv(3));
    assert_eq!(fresh.value("p2"), Tv(4));

    // 再用 apply_next 把第 3、4 条（seq 2、3）重放上去 —— undo/redo 共用的同一个函数。
    let remaining: Vec<_> = history.entries().skip(2).cloned().collect();
    assert_eq!(remaining.len(), 2);
    let mut resolve_infallible = |k: &String| fresh.slot(k);
    apply_next(&fresh.store, &mut resolve_infallible, &remaining);

    // 与原世界逐值相等 —— 恢复 = redo，同一个函数。
    assert_eq!(fresh.value("p1"), original.value("p1"));
    assert_eq!(fresh.value("p2"), original.value("p2"));
    assert_eq!(fresh.value("d"), expected_d);
}
