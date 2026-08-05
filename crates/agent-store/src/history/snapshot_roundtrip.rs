//! 全链路验收（010 验收三条）：一张有两层 derived 的图 → 记录 → **第 2 步采集快照** →
//! 继续写到第 4 步 → 整份「存盘」（走一次 JSON）→ **全新 store、全新构图** → `restore`
//! → 快照点之后的条目用 [`apply_next`](crate::history::apply_next) 一路往前推。
//!
//! 这个文件是唯一需要同时看见 `Store` / `AtomId` / 快照 / 日志的地方（`apply_roundtrip`
//! 的同款理由），所以它单独一个文件，而不是塞进 `capture.rs`。
//!
//! **值类型用 `i64`**：它的 `Serialize` 来自 serde 自己的实现，于是本文件一行
//! `derive(...Serialize...)` 都不需要 —— 红线 4 的检查器（同一文件里既有 `Serialize`
//! 派生又出现 `AtomId`）在这里同样不可能被触发。

use std::cell::RefCell;
use std::rc::Rc;

use crate::family::AtomFamily;
use crate::history::{Entry, History, Snapshot, apply_next, capture, record_set, restore};
use crate::ids::AtomId;
use crate::store::{AtomValue, Store};

impl AtomValue for i64 {
    fn null() -> Self {
        0
    }
}

type Log = History<String, i64, u32>;

/// 这一版的槽位表；`V2` 在**中间**插了一个新槽位，于是它之后所有 `AtomId` 整体后移。
const V1: &[&str] = &["a", "b", "c"];
const V2: &[&str] = &["a", "mid", "b", "c"];

fn default_for(key: &str) -> i64 {
    match key {
        "mid" => 7,
        _ => 0,
    }
}

struct World {
    store: Store<i64>,
    fam: Rc<RefCell<AtomFamily<String>>>,
    total: AtomId,
    doubled: AtomId,
}

/// 唯一的创建路径。
fn slot(w: &World, key: &str) -> AtomId {
    w.fam
        .borrow_mut()
        .get_or_create(key.to_string(), || w.store.create_atom(default_for(key)))
}

/// 构图函数：按 `keys` 顺序建 primitive，再叠两层 derived（`total` 按逻辑键现查 family，
/// `doubled` 吃 `total`）。**每次调用都是一个全新的进程该有的样子**：新 store、新 family。
fn build(keys: &'static [&'static str]) -> World {
    let store: Store<i64> = Store::new();
    let fam: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));
    for key in keys {
        fam.borrow_mut()
            .get_or_create((*key).to_string(), || store.create_atom(default_for(key)));
    }
    let (st, fm) = (store.clone(), fam.clone());
    let total = store.create_derived_ctx(move |args| {
        keys.iter()
            .map(|key| {
                let id = fm
                    .borrow_mut()
                    .get_or_create((*key).to_string(), || st.create_atom(default_for(key)));
                args.get(id)
            })
            .sum::<i64>()
    });
    let doubled = store.create_derived_ctx(move |args| args.get(total) * 2);
    let w = World {
        store,
        fam,
        total,
        doubled,
    };
    let _ = w.store.get(w.doubled); // 建立两层反向依赖边
    w
}

/// 一条 command：一次 batch → 一个 undo 步。
fn command(w: &World, log: &mut Log, turn: u32, writes: &[(&str, i64)]) {
    let mut changes = Vec::new();
    w.store.batch(|s| {
        for (key, next) in writes {
            let id = slot(w, key);
            changes.extend(record_set(s, (*key).to_string(), id, *next));
        }
    });
    log.append(turn, changes);
}

/// 上层的采集口：family 全遍历（键序不定 —— family 内部是 `HashMap`），排序后交给
/// `capture`，于是落盘字节逐字节确定。
fn capture_all(w: &World) -> Snapshot<String, i64> {
    let mut pairs: Vec<(String, AtomId)> = w
        .fam
        .borrow()
        .iter()
        .map(|(k, id)| (k.clone(), id))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    capture(&w.store, pairs.into_iter())
}

/// 灌回，返回「这一版不认识」的键。`resolve` 是**非创建**查找（`capture.rs` 里说的
/// 那处与 applier 的分岔）。
fn restore_into(w: &World, snap: &Snapshot<String, i64>) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut resolve = |k: &String| w.fam.borrow().get(k);
    restore(&w.store, &mut resolve, snap, &mut |k: &String| {
        unknown.push(k.clone())
    });
    unknown
}

/// 全部 primitive（按键序）+ 两层 derived 的当前值。
fn values(w: &World) -> Vec<i64> {
    let mut keys: Vec<String> = w.fam.borrow().iter().map(|(k, _)| k.clone()).collect();
    keys.sort();
    let mut out: Vec<i64> = keys.iter().map(|k| w.store.get(slot(w, k))).collect();
    out.push(w.store.get(w.total));
    out.push(w.store.get(w.doubled));
    out
}

/// 世界 A：四步，第 2 步之后采一份快照。返回 `(快照, 日志, 第 4 步之后的逐值)`。
fn world_a() -> (Snapshot<String, i64>, Log, Vec<i64>) {
    let a = build(V1);
    let mut log = Log::new();
    command(&a, &mut log, 1, &[("a", 10)]);
    command(&a, &mut log, 1, &[("b", 20)]);
    let snap = capture_all(&a); // ← 快照点：第 2 步之后
    command(&a, &mut log, 2, &[("c", 30)]);
    command(&a, &mut log, 2, &[("a", 100), ("b", 200)]);
    assert_eq!(values(&a), vec![100, 200, 30, 330, 660]);
    (snap, log, values(&a))
}

/// 一次「落盘 → 载入」之后拿到的东西：快照、全部条目、以及 seq 从哪继续。
type Loaded = (Snapshot<String, i64>, Vec<Entry<String, i64, u32>>, u64);

/// 「存盘 → 新进程载入」：快照与日志各走一次 JSON。落盘件的形状就是
/// `SessionStore::load` 的返回值 `(Snapshot, Vec<Entry>, cursor)`（`docs/STATE-MODEL.md`）。
fn through_disk(snap: &Snapshot<String, i64>, log: Log) -> Loaded {
    let snap_bytes = serde_json::to_string(snap).unwrap();
    let (entries, _cursor, next_seq) = log.to_parts();
    let log_bytes = serde_json::to_string(&entries).unwrap();
    (
        serde_json::from_str(&snap_bytes).unwrap(),
        serde_json::from_str(&log_bytes).unwrap(),
        next_seq,
    )
}

#[test]
fn a_snapshot_plus_the_entries_after_it_rebuilds_the_world_in_a_fresh_store() {
    // 验收 1：快照存盘 → 新 store 全新构图 → restore → 所有 derived 重算后逐值相等。
    let (snap, log, after) = world_a();
    let (snap, entries, _) = through_disk(&snap, log);

    let b = build(V1);
    assert!(restore_into(&b, &snap).is_empty());
    // 快照点之后的两条：**字面调用 apply_next**，那正是 redo 用的同一个函数。
    let mut resolve = |k: &String| slot(&b, k);
    apply_next(&b.store, &mut resolve, &entries[2..]);

    assert_eq!(values(&b), after);
}

#[test]
fn replaying_after_restore_is_literally_the_redo_path() {
    // 验收 3：恢复路径与 redo 走同一个函数。左边直接 apply_next，右边把游标摆在快照点
    // 之后调 redo_one —— 两条路殊途同归，因为 redo 的产物最终也是喂给 apply_next。
    let (snap, log, after) = world_a();
    let (snap, entries, next_seq) = through_disk(&snap, log);

    let direct = build(V1);
    assert!(restore_into(&direct, &snap).is_empty());
    let mut resolve = |k: &String| slot(&direct, k);
    apply_next(&direct.store, &mut resolve, &entries[2..]);

    let via_redo = build(V1);
    assert!(restore_into(&via_redo, &snap).is_empty());
    // 落盘件里快照与游标是配套的：游标就摆在快照点，剩下的靠 redo 推上去。
    let mut log = History::from_parts(entries, 2, next_seq).unwrap();
    while log.can_redo() {
        let outcome = log.redo_one();
        let batch = match &outcome {
            crate::history::UndoOutcome::Applied(es) => es.clone(),
            _ => panic!("redo 到顶之前不该有别的结果"),
        };
        let mut resolve = |k: &String| slot(&via_redo, k);
        apply_next(&via_redo.store, &mut resolve, &batch);
    }

    assert_eq!(values(&direct), after);
    assert_eq!(values(&via_redo), after);
    assert_eq!(log.cursor(), 4);
    // 续铸不重号：接着写下一步是 seq 4，不是从 entries.len() 反推。
    assert_eq!(
        log.append(
            3,
            vec![crate::history::Change {
                key: "a".into(),
                prev: 100,
                next: 1
            }]
        ),
        Some(4)
    );
}

#[test]
fn an_atom_inserted_in_the_middle_of_the_build_function_does_not_break_an_old_snapshot() {
    // 验收 2：构图函数中间插一个新 atom，旧快照仍正确恢复。
    let (snap, log, _) = world_a();
    let (snap, entries, _) = through_disk(&snap, log);

    let a_old = build(V1);
    let b = build(V2); // ← 中间插了 "mid"
    // 快照要是存 AtomId，这里就已经整体错位了：同一个逻辑键在两版图里的 id 不同，
    // 而且不报错。这一行就是红线 4 的全部理由。
    assert_ne!(slot(&a_old, "c"), slot(&b, "c"));

    assert!(restore_into(&b, &snap).is_empty()); // 旧键全灌回，无 panic
    let mut resolve = |k: &String| slot(&b, k);
    apply_next(&b.store, &mut resolve, &entries[2..]);

    assert_eq!(b.store.get(slot(&b, "mid")), 7); // 新槽位取构图函数给的默认值
    for (key, want) in [("a", 100), ("b", 200), ("c", 30)] {
        assert_eq!(b.store.get(slot(&b, key)), want, "{key}");
    }
    assert_eq!(b.store.get(b.total), 337); // 330 + 新槽位的 7
    assert_eq!(b.store.get(b.doubled), 674);
}

#[test]
fn a_slot_this_version_dropped_is_reported_not_silently_lost() {
    // 验收 3 的另一半：删掉的 slot → 旧快照里多出来的键交给 on_unknown，其余照常。
    let old = build(V2);
    let mut log = Log::new();
    command(&old, &mut log, 1, &[("a", 1), ("mid", 5), ("c", 9)]);
    let (snap, _, _) = through_disk(&capture_all(&old), log);

    let now = build(V1); // 这一版没有 "mid" 了
    assert_eq!(restore_into(&now, &snap), vec!["mid".to_string()]);

    assert_eq!(values(&now), vec![1, 0, 9, 10, 20]); // a / b / c / total / doubled
    assert!(now.fam.borrow().get(&"mid".to_string()).is_none()); // 没被凭空建出来
}
