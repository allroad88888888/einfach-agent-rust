//! 011 验收「写 5 entry + 1 snapshot（第 3 条后）+ 2 entry → load 得 snapshot + 之后
//! 2 条 + cursor/next_seq 正确」与「写入 → 进程重启 → 载入 → 恢复」的全链路版本：
//! 不是直接摆弄 `SessionLog`（那是 `session_log_replay.rs` 干的事），而是驱动一个
//! **真的** `History` + 真的 `store::Store`，把它的写入原样转发进 `Memory`（走
//! `SessionStore` 端口，不碰 `SessionLog` 的内部字段），再验证「新 world + `restore` +
//! `apply_next`」重建出来的状态与原 world 逐值相等——手法照抄
//! `history/snapshot_roundtrip.rs`。
//!
//! 这里还特意让 cap 驱逐和快照压实撞在一起（[`SessionLog`](agent_store::persist::SessionLog)
//! 模块文档点名的那条推导），因为这正是「转发 `DropEvent` 给持久化端口」在真实调用点
//! 会遇到的顺序，`session_log_replay.rs` 里手搓的场景只证明了公式本身对，这里证明
//! **把公式接到真 `History` 上之后还对**。

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::history::{History, UndoOutcome, apply_next, capture, record_set, restore};
use agent_store::{AtomFamily, AtomId, AtomValue, Memory, SessionStore, Snapshot, Store};

/// 值类型包一层新类型——`AtomValue` 是外部 crate 的 trait，`i64` 是原生类型，
/// `tests/` 下每个文件都是独立 crate，孤儿规则不让直接 `impl AtomValue for i64`
/// （`history/snapshot_roundtrip.rs` 能这么写是因为它长在 `agent_store` 内部）。
#[derive(Clone, Copy, Debug, PartialEq)]
struct Val(i64);

impl AtomValue for Val {
    fn null() -> Self {
        Val(0)
    }
}

type Log = History<String, Val, u32>;
type Backend = Memory<String, Val, u32>;

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

/// 一条 command：写 → 记进 `History` → **原样转发进 `SessionStore`**（`append` +
/// `set_cursor`，跟 027 将来接线时要做的事一模一样），再把 `take_drop_events` 转发
/// 给 `drop_oldest`/`drop_after`。
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
fn three_entries_a_snapshot_two_more_entries_load_matches_the_issues_acceptance_text() {
    let world = build(KEYS);
    let mut log = Log::new();
    let backend: Backend = Memory::new();

    command(&world, &mut log, &backend, 1, &[("a", Val(1))]); // seq 0
    command(&world, &mut log, &backend, 2, &[("a", Val(2))]); // seq 1
    command(&world, &mut log, &backend, 3, &[("b", Val(3))]); // seq 2 —— 第 3 条

    backend.snapshot(&capture_all(&world)); // 快照点

    command(&world, &mut log, &backend, 4, &[("a", Val(4))]); // seq 3
    command(&world, &mut log, &backend, 5, &[("b", Val(5))]); // seq 4

    let loaded = backend.load().loaded().expect("写过东西之后 load 不该是 None");
    assert!(loaded.snapshot.is_some());
    assert_eq!(loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![3, 4]);
    assert_eq!(loaded.cursor, 2); // 没 undo 过，顶
    assert_eq!(loaded.next_seq, 5);

    // 用它重建：新 world + restore 快照 + apply_next 剩下两条 —— 与原 world 逐值相等。
    let fresh = build(KEYS);
    assert!(restore_into(&fresh, &loaded.snapshot.unwrap()).is_empty());
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_next(&fresh.store, &mut resolve, &loaded.entries);
    assert_eq!(values(&fresh, KEYS), values(&world, KEYS));
}

/// 「写入 → 进程重启 → 载入 → 恢复」：cap 驱逐（`set_cap`）与快照压实撞在一起——
/// 这正是 `SessionLog::record_drop_oldest` 文档点名的那条推导在真 `History` 上的样子。
#[test]
fn cap_eviction_crossing_a_snapshot_boundary_still_recovers_the_exact_live_state() {
    let world = build(KEYS);
    let mut log = Log::new();
    log.set_cap(Some(2)); // 逼近溢出：满 2 条就要开始丢最老的
    let backend: Backend = Memory::new();

    command(&world, &mut log, &backend, 1, &[("a", Val(1))]); // seq 0
    backend.snapshot(&capture_all(&world)); // 快照压实：boundary 前移，seq 0 被吃掉

    command(&world, &mut log, &backend, 2, &[("a", Val(2))]); // seq 1，entries=[0,1]（cap 未溢出）
    command(&world, &mut log, &backend, 3, &[("b", Val(3))]); // seq 2 → 溢出，cap 驱逐 seq 0
    // 驱逐发生在 History 自己的 entries 里；backend 这边 seq 0 早被快照吃过了，
    // 这次驱逐对 backend.held 应该是空转（held 里只有 seq1，不该被误删）。
    command(&world, &mut log, &backend, 4, &[("a", Val(4))]); // seq 3 → 再溢出，驱逐 seq 1（真删）

    assert_eq!(log.entries().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]); // History 自己也只剩 2 条

    let loaded = backend.load().loaded().unwrap();
    // backend 这边：seq1 在“第二次驱逐”里被真的删掉，剩 seq2、seq3。
    assert_eq!(loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![2, 3]);
    assert_eq!(loaded.cursor, log.cursor());
    assert_eq!(loaded.next_seq, 4);

    // 恢复：新 world + restore + apply_next，与「杀掉进程重启」拿到的状态逐值相等。
    let fresh = build(KEYS);
    assert!(restore_into(&fresh, &loaded.snapshot.unwrap()).is_empty());
    let mut resolve = |k: &String| slot(&fresh, k);
    apply_next(&fresh.store, &mut resolve, &loaded.entries);
    assert_eq!(values(&fresh, KEYS), values(&world, KEYS));

    // 恢复出来的还是一份能正常 undo 的 History——`from_parts` 接住不变量，
    // `apply_prev` 走的是 undo/redo 共用的同一条路。
    let mut restored_log = Log::from_parts(loaded.entries, loaded.cursor, loaded.next_seq).unwrap();
    let outcome = restored_log.undo_one(|_| false);
    let applied = match &outcome {
        UndoOutcome::Applied(es) => es.clone(),
        other => panic!("unexpected {other:?}"),
    };
    let mut resolve = |k: &String| slot(&fresh, k);
    agent_store::apply_prev(&fresh.store, &mut resolve, &applied);
    assert_eq!(fresh.store.get(slot(&fresh, "a")), Val(2)); // 退掉 seq3（a: 4→2）
}

// 011 原文「per-session 选后端：同一段调用方代码分别喂 Memory 和 Jsonl」的完整版本
// 需要两个实现都在场——两个都在场的地方是 agent-runtime（`Jsonl` 住那），见那边的
// `session_store_backend_choice.rs`：同一个泛型 `drive_session::<S: SessionStore<..>>`
// 分别用 `Memory` 和 `Jsonl` 实例化跑一遍，断言 `load()` 的结果形状一致。
