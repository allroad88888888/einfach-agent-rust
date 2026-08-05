//! 011 验收「写入 → 进程重启 → 载入 → 恢复」，真实文件版本——手法照抄
//! `agent-store/tests/snapshot_recovery_is_redo.rs`：capture 于中途、继续写、
//! 从磁盘载入、`apply_next` 重放，与原世界逐值相等。
//!
//! 「进程重启」在这里字面地做：第一个 `Jsonl` 实例写完之后**整个 drop 掉**
//! （触发 `Drop` 里的排干），构造第二个全新实例指向同一个路径——第二个实例
//! 没有任何活体镜像，`load()` 只能靠文件内容重建，这正是这个测试要证的事。

mod session_store_support;

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::history::{
    History, UndoOutcome, apply_next, apply_prev, capture, record_set, restore,
};
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
fn kill_and_restart_recovers_the_exact_live_state_from_disk_alone() {
    let path = temp_path("crash-recovery");
    let world = build(KEYS);
    let mut log = Log::new();

    // ---- 「进程 1」：写一段，中途落一张快照，再写两步，然后整个进程消失。----
    {
        let (errors, on_error) = collecting_on_error();
        let backend: Backend = Jsonl::new(&path, on_error);
        command(&world, &mut log, &backend, 1, &[("a", Val(10))]);
        command(&world, &mut log, &backend, 2, &[("b", Val(20))]);
        backend.snapshot(&capture_all(&world));
        command(&world, &mut log, &backend, 3, &[("a", Val(100))]);
        command(&world, &mut log, &backend, 4, &[("b", Val(200))]);
        assert!(errors.lock().unwrap().is_empty());
        // `backend` 在这个块结束时被 drop——`Drop` 排干队列，文件里此刻是完整的。
    }
    assert_eq!(values(&world, KEYS), vec![Val(100), Val(200), Val(300)]);

    // ---- 「进程 2」：全新实例，同一路径，没有任何活体记忆。 ----
    let (errors2, on_error2) = collecting_on_error();
    let restarted: Backend = Jsonl::new(&path, on_error2);
    let loaded = restarted.load().loaded().expect("磁盘上明明写过东西");
    assert!(errors2.lock().unwrap().is_empty(), "干净的文件不该报错");
    assert!(loaded.snapshot.is_some());
    assert_eq!(
        loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3]
    );
    assert_eq!(loaded.next_seq, 4);

    let fresh = build(KEYS);
    assert!(restore_into(&fresh, &loaded.snapshot.clone().unwrap()).is_empty());
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_next(&fresh.store, &mut resolve, &loaded.entries);
    assert_eq!(values(&fresh, KEYS), values(&world, KEYS));

    // 恢复出来的还是一份能正常 undo 的 History——`from_parts` 接住不变量，undo/redo
    // 走的是同一条 applier 路径，不是这个测试另写的重放逻辑。
    let mut restored_log = Log::from_parts(loaded.entries, loaded.cursor, loaded.next_seq).unwrap();
    let outcome = restored_log.undo_one(|_| false);
    let applied = match &outcome {
        UndoOutcome::Applied(es) => es.clone(),
        other => panic!("unexpected {other:?}"),
    };
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_prev(&fresh.store, &mut resolve, &applied);
    assert_eq!(fresh.store.get(slot(&fresh, "b")), Val(20)); // 退掉「进程 1」的第 4 步
}
