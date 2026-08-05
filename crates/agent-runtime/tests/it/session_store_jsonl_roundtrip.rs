//! 011 验收，`Jsonl` 版本：「写 5 entry + 1 snapshot（第 3 条后）+ 2 entry → load 得
//! snapshot + 之后 2 条 + cursor/next_seq 正确」。跟 `agent-store` 里的
//! `session_store_memory_full_chain.rs` 是同一段调用方代码（`command` 帮助函数逐字
//! 相同），只换了后端——这正是 011 要求的「Memory 与 Jsonl 都过同一套端口行为测试」。

mod session_store_support;

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::history::{History, apply_next, capture, record_set, restore};
use agent_store::{AtomFamily, AtomId, SessionStore, Snapshot, Store};

use agent_runtime::Jsonl;
use session_store_support::{Val, collecting_on_error, temp_path};

type Log = History<String, Val, u32>;
type Backend = Jsonl<String, Val, u32>;

struct World {
    store: Store<Val>,
    fam: Rc<RefCell<AtomFamily<String>>>,
    total: AtomId,
}

fn slot(w: &World, key: &str) -> AtomId {
    w.fam
        .borrow_mut()
        .get_or_create(key.to_string(), || w.store.create_atom(Val(0)))
}

fn build(keys: &'static [&'static str]) -> World {
    let store: Store<Val> = Store::new();
    let fam: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));
    for key in keys {
        fam.borrow_mut()
            .get_or_create((*key).to_string(), || store.create_atom(Val(0)));
    }
    let (st, fm) = (store.clone(), fam.clone());
    let total = store.create_derived_ctx(move |args| {
        Val(keys
            .iter()
            .map(|key| {
                let id = fm
                    .borrow_mut()
                    .get_or_create((*key).to_string(), || st.create_atom(Val(0)));
                args.get(id).0
            })
            .sum::<i64>())
    });
    let w = World { store, fam, total };
    let _ = w.store.get(w.total);
    w
}

fn command(w: &World, log: &mut Log, backend: &Backend, turn: u32, writes: &[(&str, Val)]) {
    let mut changes = Vec::new();
    w.store.batch(|s| {
        for (key, next) in writes {
            let id = slot(w, key);
            changes.extend(record_set(s, (*key).to_string(), id, *next));
        }
    });
    if log.append(turn, changes).is_some() {
        backend.append(log.last().unwrap());
        backend.set_cursor(log.cursor());
    }
    for ev in log.take_drop_events() {
        match ev {
            agent_store::DropEvent::Oldest { count } => backend.drop_oldest(count),
            agent_store::DropEvent::RedoTail { first_seq, count } => {
                backend.drop_after(first_seq, count)
            }
        }
    }
}

fn capture_all(w: &World) -> Snapshot<String, Val> {
    let mut pairs: Vec<(String, AtomId)> = w
        .fam
        .borrow()
        .iter()
        .map(|(k, id)| (k.clone(), id))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    capture(&w.store, pairs.into_iter())
}

fn restore_into(w: &World, snap: &Snapshot<String, Val>) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut resolve = |k: &String| w.fam.borrow().get(k);
    restore(&w.store, &mut resolve, snap, &mut |k: &String| {
        unknown.push(k.clone())
    });
    unknown
}

fn values(w: &World, keys: &[&str]) -> Vec<Val> {
    let mut out: Vec<Val> = keys.iter().map(|k| w.store.get(slot(w, k))).collect();
    out.push(w.store.get(w.total));
    out
}

const KEYS: &[&str] = &["a", "b"];

#[test]
fn three_entries_a_snapshot_two_more_entries_load_matches_the_issues_acceptance_text() {
    let world = build(KEYS);
    let mut log = Log::new();
    let (errors, on_error) = collecting_on_error();
    let backend: Backend = Jsonl::new(temp_path("roundtrip"), on_error);

    command(&world, &mut log, &backend, 1, &[("a", Val(1))]); // seq 0
    command(&world, &mut log, &backend, 2, &[("a", Val(2))]); // seq 1
    command(&world, &mut log, &backend, 3, &[("b", Val(3))]); // seq 2 —— 第 3 条

    backend.snapshot(&capture_all(&world)); // 快照点，触发文件压实

    command(&world, &mut log, &backend, 4, &[("a", Val(4))]); // seq 3
    command(&world, &mut log, &backend, 5, &[("b", Val(5))]); // seq 4

    let loaded = backend
        .load()
        .loaded()
        .expect("写过东西之后 load 不该是 None");
    assert!(loaded.snapshot.is_some());
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 4]
    );
    assert_eq!(loaded.cursor, 2); // 没 undo 过，顶
    assert_eq!(loaded.next_seq, 5);
    assert!(
        errors.lock().unwrap().is_empty(),
        "正常路径不该报任何 on_error"
    );

    // 用它重建：新 world + restore 快照 + apply_next 剩下两条 —— 与原 world 逐值相等。
    let fresh = build(KEYS);
    assert!(restore_into(&fresh, &loaded.snapshot.unwrap()).is_empty());
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_next(&fresh.store, &mut resolve, &loaded.entries);
    assert_eq!(values(&fresh, KEYS), values(&world, KEYS));
}
