//! 全链路自测（017 验收第一条）：一张有 derived 的图 → `record_set` 记录 → `undo_turn`
//! → **调用方的 applier** 把产物写回 store → `redo_turn` 追回去。
//!
//! 这里演示的分工是本 issue 的核心判断：**History 不碰 store**。它只挪游标、把该应用的
//! 条目克隆出来；写回状态的那三行（下面的 `apply_undo` / `apply_redo`）属于上层。
//! 真实实现里 `resolve` 是 `AtomFamily` 的按键查找，已被 evict 的 atom 怎么按需重建是
//! 019；红线 6「undo 时 bump session epoch」是集成层的事，History 对 epoch 不可见。

use crate::history::{Change, Entry, History, UndoOutcome, record_set};
use crate::ids::AtomId;
use crate::store::{AtomValue, Store};

#[derive(Clone, Debug, PartialEq)]
enum TestValue {
    Num(i64),
    Text(String),
}

impl AtomValue for TestValue {
    fn null() -> Self {
        TestValue::Num(0)
    }
}

fn n(v: i64) -> TestValue {
    TestValue::Num(v)
}

fn as_num(v: &TestValue) -> i64 {
    match v {
        TestValue::Num(n) => *n,
        TestValue::Text(_) => 0,
    }
}

/// 上层的元数据。agent 侧是 `turn_id` / `label` / `Reversibility`；History 只当它是 `M`。
#[derive(Clone, Debug, PartialEq)]
struct Meta {
    turn: u32,
    label: &'static str,
    irreversible: bool,
}

fn meta(turn: u32, label: &'static str) -> Meta {
    Meta {
        turn,
        label,
        irreversible: false,
    }
}

fn same_turn(a: &Meta, b: &Meta) -> bool {
    a.turn == b.turn
}

fn barrier(m: &Meta) -> bool {
    m.irreversible
}

fn open(_: &Meta) -> bool {
    false
}

type Log = History<String, TestValue, Meta>;

/// 三个 primitive + 两层 derived：`total = a + b + c`，`banner` 再吃 `total`。
struct Graph {
    a: AtomId,
    b: AtomId,
    c: AtomId,
    total: AtomId,
    banner: AtomId,
}

fn build(store: &Store<TestValue>) -> Graph {
    let (a, b, c) = (
        store.create_atom(n(1)),
        store.create_atom(n(2)),
        store.create_atom(n(3)),
    );
    let total = store.create_derived_ctx(move |args| {
        n(as_num(&args.get(a)) + as_num(&args.get(b)) + as_num(&args.get(c)))
    });
    let banner = store.create_derived_ctx(move |args| {
        TestValue::Text(format!("total={}", as_num(&args.get(total))))
    });
    assert_eq!(store.get(banner), TestValue::Text("total=6".into())); // 建立反向依赖边
    Graph {
        a,
        b,
        c,
        total,
        banner,
    }
}

/// 逻辑键 → 进程内句柄。真实实现是 `AtomFamily::get_or_create(AtomKey)`，日志里存的
/// 始终是逻辑键（红线 4）—— 这个函数就是「日志侧永远看不见 `AtomId`」的接缝。
fn resolve(g: &Graph, key: &str) -> AtomId {
    match key {
        "a" => g.a,
        "b" => g.b,
        "c" => g.c,
        other => panic!("未知逻辑键 {other}"),
    }
}

/// 一条 command：一次 batch 里若干 primitive 写入 → 一个 undo 步。
fn command(
    store: &Store<TestValue>,
    log: &mut Log,
    g: &Graph,
    m: Meta,
    writes: &[(&str, TestValue)],
) {
    let mut changes: Vec<Change<String, TestValue>> = Vec::new();
    store.batch(|s| {
        for (key, next) in writes {
            changes.extend(record_set(
                s,
                (*key).to_string(),
                resolve(g, key),
                next.clone(),
            ));
        }
    });
    log.append(m, changes);
}

fn applied(outcome: &UndoOutcome<String, TestValue, Meta>) -> &[Entry<String, TestValue, Meta>] {
    match outcome {
        UndoOutcome::Applied(es) | UndoOutcome::Blocked { applied: es, .. } => es,
        UndoOutcome::Nothing => &[],
    }
}

/// **调用方的 applier**：条目已按 seq 倒序给好，每条内部的 `changes` 再倒序写 `prev`。
fn apply_undo(store: &Store<TestValue>, g: &Graph, outcome: &UndoOutcome<String, TestValue, Meta>) {
    store.batch(|s| {
        for entry in applied(outcome) {
            for change in entry.changes.iter().rev() {
                s.set(resolve(g, &change.key), change.prev.clone());
            }
        }
    });
}

/// redo 方向：条目正序、`changes` 正序、写 `next`。
fn apply_redo(store: &Store<TestValue>, g: &Graph, outcome: &UndoOutcome<String, TestValue, Meta>) {
    store.batch(|s| {
        for entry in applied(outcome) {
            for change in &entry.changes {
                s.set(resolve(g, &change.key), change.next.clone());
            }
        }
    });
}

/// 全部 primitive 的逐值快照 + 两个 derived 的当前值。
fn snapshot(store: &Store<TestValue>, g: &Graph) -> Vec<TestValue> {
    vec![
        store.get(g.a),
        store.get(g.b),
        store.get(g.c),
        store.get(g.total),
        store.get(g.banner),
    ]
}

#[test]
fn undo_turn_then_redo_turn_restores_every_primitive_and_derived() {
    let store: Store<TestValue> = Store::new();
    let g = build(&store);
    let mut log = Log::new();

    // turn 1：两条 command。
    command(&store, &mut log, &g, meta(1, "set_a"), &[("a", n(10))]);
    command(&store, &mut log, &g, meta(1, "set_b"), &[("b", n(20))]);
    let after_turn_1 = snapshot(&store, &g);
    assert_eq!(after_turn_1[3], n(33)); // 10 + 20 + 3

    // turn 2：又两条，其中一条一次改两个 primitive。
    command(&store, &mut log, &g, meta(2, "set_c"), &[("c", n(100))]);
    command(
        &store,
        &mut log,
        &g,
        meta(2, "set_ab"),
        &[("a", n(1000)), ("b", n(2000))],
    );
    let after_turn_2 = snapshot(&store, &g);
    assert_eq!(after_turn_2[3], n(3100));
    assert_eq!(after_turn_2[4], TestValue::Text("total=3100".into()));
    assert_eq!(log.cursor(), 4);

    // —— undo 一整个 turn ——
    let recomputes_before = store.debug_recompute_count();
    let outcome = log.undo_turn(same_turn, open);
    assert_eq!(
        applied(&outcome).iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![3, 2]
    );
    apply_undo(&store, &g, &outcome);

    assert_eq!(log.cursor(), 2);
    assert_eq!(snapshot(&store, &g), after_turn_1); // primitive 逐值相等
    assert_eq!(store.get(g.total), n(33)); // derived 重算一致，不是缓存
    assert!(store.debug_recompute_count() > recomputes_before);

    // —— redo 同一粒度，恰好反演 ——
    let outcome = log.redo_turn(same_turn);
    assert_eq!(
        applied(&outcome).iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2, 3]
    );
    apply_redo(&store, &g, &outcome);

    assert_eq!(log.cursor(), 4);
    assert!(!log.can_redo());
    assert_eq!(snapshot(&store, &g), after_turn_2);

    // 再 undo 一次，跨过 turn 1 的边界回到起点。
    apply_undo(&store, &g, &log.undo_turn(same_turn, open));
    apply_undo(&store, &g, &log.undo_turn(same_turn, open));
    assert_eq!(log.cursor(), 0);
    assert_eq!(
        snapshot(&store, &g),
        vec![n(1), n(2), n(3), n(6), TestValue::Text("total=6".into())]
    );
}

#[test]
fn a_batch_that_wrote_one_atom_twice_only_unwinds_in_reverse() {
    // 一次 batch 里 a: 1→2→3，两条 change 的 prev 链是 1→2。倒序回滚才回得到 1。
    let store: Store<TestValue> = Store::new();
    let g = build(&store);
    let mut log = Log::new();
    command(
        &store,
        &mut log,
        &g,
        meta(1, "twice"),
        &[("a", n(2)), ("a", n(3))],
    );
    assert_eq!(store.get(g.a), n(3));

    let outcome = log.undo_one(open);
    let entry = &applied(&outcome)[0];
    assert_eq!(entry.changes.len(), 1 + 1);
    apply_undo(&store, &g, &outcome);
    assert_eq!(store.get(g.a), n(1));
    assert_eq!(store.get(g.total), n(6));

    // 正序应用同一批 prev 会停在 2 —— 这就是「条目内也倒序」的全部理由。
    store.batch(|s| {
        for change in &entry.changes {
            s.set(resolve(&g, &change.key), change.prev.clone());
        }
    });
    assert_eq!(store.get(g.a), n(2));
}

#[test]
fn blocked_undo_applies_what_it_popped_and_stops_at_the_barrier() {
    let store: Store<TestValue> = Store::new();
    let g = build(&store);
    let mut log = Log::new();

    command(&store, &mut log, &g, meta(1, "set_a"), &[("a", n(10))]);
    let sent_mail = Meta {
        turn: 1,
        label: "send_mail",
        irreversible: true,
    };
    command(&store, &mut log, &g, sent_mail, &[("b", n(20))]);
    command(&store, &mut log, &g, meta(1, "set_c"), &[("c", n(30))]);

    let outcome = log.undo_turn(same_turn, barrier);
    assert!(matches!(
        outcome,
        UndoOutcome::Blocked { barrier_seq: 1, .. }
    ));
    apply_undo(&store, &g, &outcome);

    // 屏障之后的那一条回滚了，屏障本身与它之前的都还在。
    assert_eq!(log.cursor(), 2);
    assert_eq!(snapshot(&store, &g)[..3], [n(10), n(20), n(3)]);
    assert_eq!(store.get(g.total), n(33));

    // 屏障没被越过 → 再问一次仍然是同样的答案，状态不再变。
    let again = log.undo_turn(same_turn, barrier);
    assert_eq!(applied(&again).len(), 0);
    assert_eq!(log.cursor(), 2);
}
