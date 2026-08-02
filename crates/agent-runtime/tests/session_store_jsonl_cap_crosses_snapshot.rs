//! `Jsonl` 版本的「cap 驱逐横跨快照压实边界」——`agent-store` 那边的
//! `session_store_memory_full_chain.rs::cap_eviction_crossing_a_snapshot_boundary_...`
//! 是同一个场景，这里额外加了一次「进程重启」：写完之后整个 `Jsonl` 实例 drop 掉，
//! 全新实例重新指向同一个文件 `load()`。
//!
//! 这个场景专门用来钉住 `io_thread.rs` 落盘前必须做的换算（模块文档「压实之后为什么
//! 不能落原始值」）——如果落盘的是调用方给的原始 `cursor`/`count` 而不是
//! `SessionLog` 自己换算过的净效果，快照截断文件之后，全新一份 `SessionLog`（`load`
//! 用的那份，`boundary` 从 0 起步）重放出来的游标会和真实的活体状态对不上，
//! 而且不 panic、不报错——正是这类静默错值最难查的地方，得有一个测试专门盯着它。

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
    w.fam.borrow_mut().get_or_create(key.to_string(), || w.store.create_atom(Val(0)))
}

fn build(keys: &'static [&'static str]) -> World {
    let store: Store<Val> = Store::new();
    let fam: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));
    for key in keys {
        fam.borrow_mut().get_or_create((*key).to_string(), || store.create_atom(Val(0)));
    }
    let (st, fm) = (store.clone(), fam.clone());
    let total = store.create_derived_ctx(move |args| {
        Val(
            keys.iter()
                .map(|key| {
                    let id = fm.borrow_mut().get_or_create((*key).to_string(), || st.create_atom(Val(0)));
                    args.get(id).0
                })
                .sum::<i64>(),
        )
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
            agent_store::DropEvent::RedoTail { first_seq, count } => backend.drop_after(first_seq, count),
        }
    }
}

fn capture_all(w: &World) -> Snapshot<String, Val> {
    let mut pairs: Vec<(String, AtomId)> = w.fam.borrow().iter().map(|(k, id)| (k.clone(), id)).collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    capture(&w.store, pairs.into_iter())
}

fn restore_into(w: &World, snap: &Snapshot<String, Val>) -> Vec<String> {
    let mut unknown = Vec::new();
    let mut resolve = |k: &String| w.fam.borrow().get(k);
    restore(&w.store, &mut resolve, snap, &mut |k: &String| unknown.push(k.clone()));
    unknown
}

fn values(w: &World, keys: &[&str]) -> Vec<Val> {
    let mut out: Vec<Val> = keys.iter().map(|k| w.store.get(slot(w, k))).collect();
    out.push(w.store.get(w.total));
    out
}

const KEYS: &[&str] = &["a", "b"];

#[test]
fn cap_eviction_crossing_a_snapshot_boundary_survives_a_restart() {
    let path = temp_path("cap-crosses-snapshot");
    let world = build(KEYS);
    let mut log = Log::new();
    log.set_cap(Some(2));

    {
        let (errors, on_error) = collecting_on_error();
        let backend: Backend = Jsonl::new(&path, on_error);

        command(&world, &mut log, &backend, 1, &[("a", Val(1))]); // seq 0
        backend.snapshot(&capture_all(&world)); // 压实：文件截断只剩这张快照

        command(&world, &mut log, &backend, 2, &[("a", Val(2))]); // seq 1，未溢出
        command(&world, &mut log, &backend, 3, &[("b", Val(3))]); // seq 2 → 溢出，驱逐 seq0
        // seq0 早被快照吃过了：这次驱逐对 held 应该是空转（跟 Memory 那边的推导一样，
        // 只是这里额外经过一次「写文件 → 从文件重放」）。
        command(&world, &mut log, &backend, 4, &[("a", Val(4))]); // seq 3 → 再溢出，真删 seq1

        assert!(errors.lock().unwrap().is_empty());
        // 块结束，`backend` drop——排干队列，文件落定。
    }
    assert_eq!(log.entries().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);

    let (errors2, on_error2) = collecting_on_error();
    let restarted: Backend = Jsonl::new(&path, on_error2);
    let loaded = restarted.load().loaded().unwrap();
    assert!(errors2.lock().unwrap().is_empty());

    // 这正是修复前会错的地方：压实截断文件之后，游标必须是「换算过的净效果」，
    // 不是调用方给的原始 History::cursor() 原样值——否则这里会跟 log.cursor() 对不上。
    assert_eq!(loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
    assert_eq!(loaded.cursor, log.cursor());
    assert_eq!(loaded.next_seq, 4);

    let fresh = build(KEYS);
    assert!(restore_into(&fresh, &loaded.snapshot.unwrap()).is_empty());
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_next(&fresh.store, &mut resolve, &loaded.entries);
    assert_eq!(values(&fresh, KEYS), values(&world, KEYS));
}
