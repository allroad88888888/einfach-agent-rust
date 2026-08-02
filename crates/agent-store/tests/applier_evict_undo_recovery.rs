//! 019 验收原文：evict 一个子 agent 的全部 atom，undo 回它运行中的那一刻——状态完全恢复。
//!
//! 场景：一个 `AtomFamily` 键控 3 个 primitive + 1 个 derived。record_set 写两轮
//! （两个不同的 turn），读一次 derived 建立依赖边，然后 evict 掉 derived 和其中两个
//! primitive（第三个留着不动，模拟"只有部分槽位被回收"）。undo_turn 只应该弹第二轮
//! （因为两轮分属不同 turn），落地到 apply_prev + get-or-create 闭包后，
//! 3 个 primitive 的值必须全部回到第一轮末——包括被重建的那两个。

mod common;

use std::cell::RefCell;
use std::rc::Rc;

use agent_store::{apply_prev, record_set, AtomFamily, AtomId, Entry, History, Store, UndoOutcome};
use common::{num, TestValue as V};

#[derive(Debug, Clone, PartialEq)]
struct Meta {
    turn: u32,
}

fn same_turn(a: &Meta, b: &Meta) -> bool {
    a.turn == b.turn
}

fn no_barrier(_: &Meta) -> bool {
    false
}

#[test]
fn evict_two_of_three_primitives_then_undo_turn_fully_recovers() {
    let store: Store<V> = Store::new();
    let family = Rc::new(RefCell::new(AtomFamily::<String>::new()));

    let p1 = family
        .borrow_mut()
        .get_or_create("p1".to_string(), || store.create_atom(num(0.0)));
    let p2 = family
        .borrow_mut()
        .get_or_create("p2".to_string(), || store.create_atom(num(0.0)));
    let p3 = family
        .borrow_mut()
        .get_or_create("p3".to_string(), || store.create_atom(num(0.0)));

    // derived：三个 primitive 求和。用 create_derived_ctx（懒），不读就不建依赖边。
    let d = family.borrow_mut().get_or_create("d".to_string(), || {
        store.create_derived_ctx(move |args| {
            match (args.get(p1), args.get(p2), args.get(p3)) {
                (V::Number(a), V::Number(b), V::Number(c)) => V::Number(a + b + c),
                _ => num(0.0),
            }
        })
    });

    let mut history: History<String, V, Meta> = History::new();

    // 第一轮（turn 1）：0,0,0 -> 1,2,3。
    let mut changes = Vec::new();
    store.batch(|s| {
        changes.extend(record_set(s, "p1".to_string(), p1, num(1.0)));
        changes.extend(record_set(s, "p2".to_string(), p2, num(2.0)));
        changes.extend(record_set(s, "p3".to_string(), p3, num(3.0)));
    });
    assert_eq!(history.append(Meta { turn: 1 }, changes), Some(0));

    // 读一次 derived：建立依赖边（懒求值不读就不建边），顺带证明它当时是对的。
    assert_eq!(store.get(d), num(6.0));

    // 第二轮（turn 2）：1,2,3 -> 10,20,30。
    let mut changes = Vec::new();
    store.batch(|s| {
        changes.extend(record_set(s, "p1".to_string(), p1, num(10.0)));
        changes.extend(record_set(s, "p2".to_string(), p2, num(20.0)));
        changes.extend(record_set(s, "p3".to_string(), p3, num(30.0)));
    });
    assert_eq!(history.append(Meta { turn: 2 }, changes), Some(1));

    // family 的真实 API 行为：derived 还依赖着 p1 时，evict(p1) 必须被拒绝。
    assert!(
        !family.borrow_mut().evict(&store, &"p1".to_string()),
        "derived 仍依赖 p1，evict 应当拒绝"
    );
    // 解除依赖的办法是先把 derived 自己 evict 掉——它自己没有被依赖也没有订阅者，
    // family.evict 内部走 store.destroy_atom，会顺带 sever 它对 p1/p2/p3 的依赖边。
    assert!(
        family.borrow_mut().evict(&store, &"d".to_string()),
        "derived 自身没有下游，evict 不应该被拒绝"
    );
    // 依赖边解除之后，两个 primitive 才能被回收。
    assert!(family.borrow_mut().evict(&store, &"p1".to_string()));
    assert!(family.borrow_mut().evict(&store, &"p2".to_string()));
    assert!(family.borrow().get(&"p1".to_string()).is_none());
    assert!(family.borrow().get(&"p2".to_string()).is_none());
    // p3 完全没碰过。
    assert_eq!(family.borrow().get(&"p3".to_string()), Some(p3));

    // undo 一整个 turn：两轮分属不同 turn，所以只弹第二轮。
    let outcome = history.undo_turn(same_turn, no_barrier);
    let entries: Vec<Entry<String, V, Meta>> = match outcome {
        UndoOutcome::Applied(es) => es,
        other => panic!("expected Applied, got {other:?}"),
    };
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].meta.turn, 2);
    assert_eq!(entries[0].changes.len(), 3);

    // resolve：get-or-create 闭包，被 evict 掉的按需重建（默认值 create，再由 apply_prev 灌值）。
    let family_for_resolve = family.clone();
    let store_for_resolve = store.clone();
    let mut resolve = move |k: &String| -> AtomId {
        family_for_resolve
            .borrow_mut()
            .get_or_create(k.clone(), || store_for_resolve.create_atom(num(0.0)))
    };
    apply_prev(&store, &mut resolve, &entries);

    let p1_after = family.borrow().get(&"p1".to_string()).unwrap();
    let p2_after = family.borrow().get(&"p2".to_string()).unwrap();
    let p3_after = family.borrow().get(&"p3".to_string()).unwrap();

    // 3 个 primitive 全部回到第一轮末。
    assert_eq!(store.get(p1_after), num(1.0));
    assert_eq!(store.get(p2_after), num(2.0));
    assert_eq!(store.get(p3_after), num(3.0));

    // 重建的那两个也在——而且是货真价实的新 atom（id 变了），不是碰巧还活着。
    assert_ne!(p1_after, p1);
    assert_ne!(p2_after, p2);
    // 没被 evict 的那个 id 原封不动。
    assert_eq!(p3_after, p3);
}
