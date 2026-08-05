//! 屏障演示（issue 020 验收「undo 越过一次 shell/exec 时停下并推 undo_blocked，
//! 不静默回滚」的机制证明）：这里不跑真的 shell，只证明 020 依赖的地基——
//! `agent-store`（017）的 `History::undo_turn` 撞上一条标了 irreversible 的
//! entry 时确实停在门口，不越过去。
//!
//! `srv:shell/exec` 的 reversibility 是 `Irreversible`；宿主侧的 undo command
//! 会用等价于 `Reversibility::blocks_undo()` 的谓词当 barrier 喂给
//! `undo_turn`。这个测试钉死 `History` 那一侧的行为契约，不牵涉 `agent-tools`
//! 自己的任何实现——这也是为什么它只依赖 `agent-store`（本文件是唯一需要
//! `agent-store` 这个 dev-dependency 的地方）。

use agent_store::{AtomId, AtomValue, History, Store, UndoOutcome, record_set};

/// 屏障用的最小 meta：`same_turn` 在这个演示里恒真（三条记成一个 turn），
/// 唯一有意义的字段是 `irreversible`——对应 `Reversibility::Irreversible`。
#[derive(Clone, Copy, Debug, PartialEq)]
struct M {
    irreversible: bool,
}

fn m(irreversible: bool) -> M {
    M { irreversible }
}

/// 恒真：三条 entry 算一个 turn，`undo_turn` 才会一路走到屏障跟前，而不是在
/// 第一条就因为「不同 turn」提前停下。
fn same_turn(_: &M, _: &M) -> bool {
    true
}

fn is_barrier(meta: &M) -> bool {
    meta.irreversible
}

/// 最小 `AtomValue`：只需要能装一个整数、可比较、可 clone。
#[derive(Clone, Copy, Debug, PartialEq)]
struct V(i64);

impl AtomValue for V {
    fn null() -> Self {
        V(0)
    }
}

fn write(
    store: &Store<V>,
    history: &mut History<String, V, M>,
    atom: AtomId,
    next: i64,
    meta: M,
) -> u64 {
    let change =
        record_set(store, "p".to_string(), atom, V(next)).expect("值确实变了，必须产出 Change");
    history
        .append(meta, vec![change])
        .expect("非空 changes 必须落一条 entry")
}

#[test]
fn undo_turn_stops_at_the_entry_marked_irreversible_shell_exec() {
    let store: Store<V> = Store::new();
    let p = store.create_atom(V(0));
    let mut history: History<String, V, M> = History::new();

    write(&store, &mut history, p, 1, m(false)); // e0：普通一步
    let barrier_seq = write(&store, &mut history, p, 2, m(true)); // e1：srv:shell/exec，irreversible
    write(&store, &mut history, p, 3, m(false)); // e2：普通一步

    assert_eq!(history.cursor(), 3);

    let applied = match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert_eq!(
                bs, barrier_seq,
                "barrier_seq 必须精确指向那条 irreversible 的 entry（e1）"
            );
            applied
        }
        other => panic!("expected Blocked，撞上 irreversible entry 不该静默回滚，实际：{other:?}"),
    };

    assert_eq!(
        applied.len(),
        1,
        "applied 必须恰好只含屏障之后那一条（e2），不含屏障本身"
    );
    assert_eq!(applied[0].changes[0].next, V(3), "被弹出的正是 e2 那次写入");

    // 屏障挡住了，游标停在屏障刚生效之后那一格：e0、e1（屏障本身）仍算数，e2 已弹出。
    assert_eq!(history.cursor(), 2);

    // 再撞一次：门口就是屏障，applied 空、位置不变——这是「undo_blocked」该推给
    // 用户的稳定信号，不是偶然只挡一次。
    match history.undo_turn(same_turn, is_barrier) {
        UndoOutcome::Blocked {
            applied,
            barrier_seq: bs,
        } => {
            assert!(applied.is_empty());
            assert_eq!(bs, barrier_seq);
        }
        other => panic!("expected Blocked again，屏障必须是永久的墙，实际：{other:?}"),
    }
    assert_eq!(history.cursor(), 2);
}
