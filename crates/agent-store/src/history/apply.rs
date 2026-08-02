//! applier：把 undo / redo 的产物写回 store。日志侧只认逻辑键 `K`（红线 4），进程内的
//! `AtomId` 由调用方给的 `resolve` 提供 —— 而 `resolve` 是 **get-or-create**，于是
//! 「这个 atom 早就被逐出了」在这两个函数里根本不是一种情况（019）。
//!
//! ## 重建为什么长在 `resolve` 上，而不是长在 applier 里
//!
//! applier 若自己判「atom 还在不在」，就必须知道「不在时按什么类型建、初值是什么」——
//! 那是上层 Slot 表的知识，本 crate 泛型化之后这里连默认值都只能问 `AtomValue::null()`。
//! 把重建塞进 applier = 在引擎里复刻一份上层 schema，而且是**只有 undo 路径才会走到**
//! 的那一份：它会和正常创建路径长期失同步，症状是「长会话 + 逐出 + undo」三件事同时
//! 发生才出现的静默错值。交给 `resolve` 之后，重建走的就是 family 平时建 atom 的
//! 同一行代码（测试里 applier 的 `resolve` 与 command 层用的是同一个函数）。
//!
//! 结构性后果：**这个文件里没有任何「不存在就怎样」的分支**，一个 `if` 都没有。
//!
//! **applier 只写日志里有的东西**：逐出不产生 `Change`，被逐出的槽位能否拿回活值取决于
//! teardown command 有没有把活值记成 `prev` —— 重建保证 atom 回来，不保证值回来。
//!
//! ## 顺序契约（017 定的）
//!
//! `UndoOutcome::Applied` 里的条目顺序就是应用顺序（undo 已按 `seq` 倒序、redo 正序），
//! **条目内部 `changes` 的方向由 applier 负责**：undo 倒序写 `prev`，redo 正序写 `next`。
//! 一次 batch 里同一个槽位被写两次（`0→2`、`2→5`）时，只有倒序回滚才回得到 0。
//!
//! ## 为什么整批包在一个 `batch` 里
//!
//! 一次 undo 是一次状态跃迁，中间态不该被任何 derived 看见。不批就是每写一个 primitive
//! 冲一次：下游 derived 会在「一半旧一半新」的世界上重算若干次（glitch），而且那时
//! 后面的槽位可能还没被 `resolve` 建回来。批到最后一次 flush，下游只重算一次，且是在
//! 全部值就位、缺席的 atom 全部重建之后 —— 重建的 atom 正是在这一次重算里被下游
//! derived 重新 `get` 到，从而重新接进依赖图。红线 6（bump epoch）在这两个函数之外：
//! `M` 里的字段本 crate 不认识，集成层在调 `apply_prev` 之前 bump。

use crate::ids::AtomId;
use crate::store::{AtomValue, Store};

use super::log::Entry;

/// 把 undo 产物写回 store：对每条 entry（按给定顺序），changes **倒序**逐条写 `prev`。
///
/// `resolve` 是 **get-or-create**：atom 已被逐出就按 `K` 重建再灌值 —— 上游 TS applier
/// 的 `resolve(op.scope)` 同款。重建必须走正常创建路径（`AtomFamily::get_or_create` +
/// 平时那个 create 闭包），不是特判分支，理由见模块文档。
///
/// 典型调用：`apply_prev(&store, &mut resolve, applied(&outcome))`，其中 `applied` 取
/// [`UndoOutcome`](super::UndoOutcome) 的 `Applied` / `Blocked { applied, .. }` 两支
/// —— `Blocked` 那半同样要应用（撞屏障前已经弹出来的条目）。
pub fn apply_prev<K, V: AtomValue, M>(
    store: &Store<V>,
    resolve: &mut impl FnMut(&K) -> AtomId,
    entries: &[Entry<K, V, M>],
) {
    store.batch(|s| {
        for entry in entries {
            for change in entry.changes.iter().rev() {
                s.set(resolve(&change.key), change.prev.clone());
            }
        }
    });
}

/// redo 方向：按给定顺序，changes **正序**逐条写 `next`。
///
/// 重建的必要性和 undo 方向一样真实：一个子 agent 结束后 atom 被逐出，用户 undo 回它
/// 出生之前、再 redo 回它运行中的那一刻 —— 这一步要写的槽位同样不存在。
pub fn apply_next<K, V: AtomValue, M>(
    store: &Store<V>,
    resolve: &mut impl FnMut(&K) -> AtomId,
    entries: &[Entry<K, V, M>],
) {
    store.batch(|s| {
        for entry in entries {
            for change in &entry.changes {
                s.set(resolve(&change.key), change.next.clone());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::family::AtomFamily;
    use crate::history::{History, UndoOutcome, record_set};

    #[derive(Clone, Debug, PartialEq)]
    enum Tv {
        Num(i64),
        Roster(Vec<&'static str>),
    }

    impl AtomValue for Tv {
        fn null() -> Self {
            Tv::Num(0)
        }
    }

    const ROSTER: &str = "roster";
    const TOKENS: &str = "agent/a1/tokens";
    const STEPS: &str = "agent/a1/steps";

    fn n(v: i64) -> Tv { Tv::Num(v) }
    fn as_num(v: &Tv) -> i64 {
        match v {
            Tv::Num(x) => *x,
            _ => 0,
        }
    }
    fn live_agents(v: &Tv) -> Vec<&'static str> {
        match v {
            Tv::Roster(r) => r.clone(),
            _ => Vec::new(),
        }
    }
    /// 上层 Slot 表在测试里的最小形状：默认值由**键**决定。applier 不需要、也不该知道。
    fn default_for(key: &str) -> Tv {
        match key {
            ROSTER => Tv::Roster(Vec::new()),
            _ => Tv::null(),
        }
    }

    type Fam = Rc<RefCell<AtomFamily<String>>>;
    type Log = History<String, Tv, u32>;

    struct World {
        store: Store<Tv>,
        fam: Fam,
        total: AtomId,
    }

    /// **唯一的创建路径**：command 层写槽位走它，applier 的 `resolve` 也走它，derived
    /// 现查槽位还是走它。019 的「重建」因此不是一条新路径，只是这一行第二次跑到 create。
    fn slot(w: &World, key: &str) -> AtomId {
        w.fam
            .borrow_mut()
            .get_or_create(key.to_string(), || w.store.create_atom(default_for(key)))
    }

    /// root 的活子 agent 名单 + 一个「名单里所有子 agent 的 tokens + steps 之和」derived。
    /// derived 按**逻辑键**现查槽位，这是重建能重新接进依赖图的前提：它认的是键，
    /// 不是某次创建出来的 `AtomId`。
    fn build() -> World {
        let store: Store<Tv> = Store::new();
        let fam: Fam = Rc::new(RefCell::new(AtomFamily::new()));
        let (st, fm) = (store.clone(), fam.clone());
        let roster = fm
            .borrow_mut()
            .get_or_create(ROSTER.to_string(), || st.create_atom(default_for(ROSTER)));
        let total = store.create_derived_ctx(move |args| {
            let mut sum = 0;
            for agent in live_agents(&args.get(roster)) {
                for name in ["tokens", "steps"] {
                    let key = format!("agent/{agent}/{name}");
                    let id = fm.borrow_mut().get_or_create(key.clone(), || {
                        st.create_atom(default_for(&key))
                    });
                    sum += as_num(&args.get(id));
                }
            }
            n(sum)
        });
        let w = World { store, fam, total };
        assert_eq!(w.store.get(w.total), n(0)); // 建立反向依赖边
        w
    }

    /// 一条 command：一次 batch 里若干写入 → 一个 undo 步（键不存在就地建）。
    fn command(w: &World, log: &mut Log, turn: u32, writes: &[(&str, Tv)]) {
        let mut changes = Vec::new();
        w.store.batch(|s| {
            for (key, next) in writes {
                let id = slot(w, key);
                changes.extend(record_set(s, (*key).to_string(), id, next.clone()));
            }
        });
        log.append(turn, changes);
    }

    fn applied(o: &UndoOutcome<String, Tv, u32>) -> &[Entry<String, Tv, u32>] {
        match o {
            UndoOutcome::Applied(es) | UndoOutcome::Blocked { applied: es, .. } => es,
            UndoOutcome::Nothing => &[],
        }
    }

    fn same_turn(a: &u32, b: &u32) -> bool { a == b }
    fn open(_: &u32) -> bool { false }

    #[test]
    fn an_evicted_subgraph_is_rebuilt_by_undo_and_the_derived_recomputes() {
        let w = build();
        let mut log = Log::new();

        // turn 1 spawn：子 agent 的槽位在这一步才被创建。turn 2 干活。
        command(&w, &mut log, 1, &[(ROSTER, Tv::Roster(vec!["a1"])), (TOKENS, n(3)), (STEPS, n(1))]);
        command(&w, &mut log, 2, &[(TOKENS, n(12)), (STEPS, n(4))]);
        assert_eq!(w.store.get(w.total), n(16));
        let old = (slot(&w, TOKENS), slot(&w, STEPS));

        // turn 3 收尾：清空槽位（prev 当场捕获的是活值 12 / 4）、移出名单，然后逐出。
        // 逐出必须自叶向根：名单里还有它时 derived 持着边，store 会拒绝。
        command(&w, &mut log, 3, &[(TOKENS, n(0)), (STEPS, n(0)), (ROSTER, Tv::Roster(vec![]))]);
        assert!(w.fam.borrow_mut().evict(&w.store, &TOKENS.to_string()));
        assert!(w.fam.borrow_mut().evict(&w.store, &STEPS.to_string()));
        assert!(!w.store.has_atom(old.0) && !w.store.has_atom(old.1));
        assert_eq!(w.store.get(w.total), n(0));

        // undo 回它运行中的那一刻 —— 两个 atom 都不在了，applier 一视同仁地 resolve。
        let recomputes = w.store.debug_recompute_count();
        let outcome = log.undo_turn(same_turn, open);
        let mut resolve = |k: &String| slot(&w, k);
        apply_prev(&w.store, &mut resolve, applied(&outcome));
        // 整批一个 batch：下游只在全部值就位、缺席的 atom 全部重建之后重算一次。
        assert_eq!(w.store.debug_recompute_count() - recomputes, 1);

        let new = (slot(&w, TOKENS), slot(&w, STEPS));
        assert!(new.0 != old.0 && new.1 != old.1); // 新 atom：id 不复用
        assert_eq!((w.store.get(new.0), w.store.get(new.1)), (n(12), n(4))); // 状态完全恢复
        assert_eq!(w.store.get(w.total), n(16)); // 下游 derived 重算，不是停在 0
        assert!(w.store.has_dependents(new.0)); // 重建的 atom 真的接回了依赖图
        // 接回来的边是活的：再写一次，derived 跟着走。
        assert!(record_set(&w.store, TOKENS.to_string(), new.0, n(100)).is_some());
        assert_eq!(w.store.get(w.total), n(104));
    }

    #[test]
    fn apply_next_rebuilds_what_redo_needs_too() {
        let w = build();
        let mut log = Log::new();
        command(&w, &mut log, 1, &[(ROSTER, Tv::Roster(vec!["a1"])), (TOKENS, n(3)), (STEPS, n(1))]);
        assert_eq!(w.store.get(w.total), n(4));

        // 退回子 agent 出生之前（名单空了 → 槽位没人依赖），再把这个子图整个逐出。
        let outcome = log.undo_turn(same_turn, open);
        let mut resolve = |k: &String| slot(&w, k);
        apply_prev(&w.store, &mut resolve, applied(&outcome));
        assert!(w.fam.borrow_mut().evict(&w.store, &TOKENS.to_string()));
        assert!(w.fam.borrow_mut().evict(&w.store, &STEPS.to_string()));

        // redo 回它运行中的那一刻：要写的槽位同样不存在，同一个 resolve 把它建回来。
        let outcome = log.redo_turn(same_turn);
        let mut resolve = |k: &String| slot(&w, k);
        apply_next(&w.store, &mut resolve, applied(&outcome));
        assert_eq!(w.store.get(slot(&w, TOKENS)), n(3));
        assert_eq!(w.store.get(w.total), n(4));
    }

    #[test]
    fn a_slot_the_derived_still_reads_cannot_be_evicted_at_all() {
        // 「整个子图逐出」是自叶向根的：还有下游时 `evict` 返回 false（`destroy_atom`
        // 更直接 panic）。于是「重建后旧 derived 会不会停在旧值」这个问题在本 store 里
        // 不成立 —— 带边的 derived 根本活不到重建那一刻。
        let w = build();
        let mut log = Log::new();
        command(&w, &mut log, 1, &[(ROSTER, Tv::Roster(vec!["a1"])), (TOKENS, n(3))]);
        assert!(w.store.has_dependents(slot(&w, TOKENS)));
        assert!(!w.fam.borrow_mut().evict(&w.store, &TOKENS.to_string()));

        command(&w, &mut log, 2, &[(ROSTER, Tv::Roster(vec![]))]);
        assert!(w.fam.borrow_mut().evict(&w.store, &TOKENS.to_string()));
    }

    #[test]
    #[should_panic(expected = "not found in store")]
    fn a_derived_that_captured_an_atom_id_does_not_reconnect() {
        // 重连的前提是 derived 按**键**现查（`build` 里的 total 就是）。捕获了 `AtomId`
        // 的 derived 在重建后指着一个死槽位；id 不复用，所以它不会静默读到别人的值，
        // 而是当场 panic。这条是「状态搬进原子图」时 derived 怎么组织的硬约束。
        let w = build();
        let id = slot(&w, TOKENS);
        let stuck = w.store.create_derived_ctx(move |args| args.get(id)); // 没读过 → 没有边
        assert!(w.fam.borrow_mut().evict(&w.store, &TOKENS.to_string()));
        assert_ne!(slot(&w, TOKENS), id); // 按键重建 = 一个新 atom
        let _ = w.store.get(stuck);
    }

    #[test]
    fn changes_inside_one_entry_unwind_in_reverse_and_redo_forward() {
        // 一次 batch 里同一个槽位写两次（0→2→5），prev 链是 0→2：倒序才回得到 0。
        let w = build();
        let mut log = Log::new();
        command(&w, &mut log, 1, &[(ROSTER, Tv::Roster(vec!["a1"])), (TOKENS, n(2)), (TOKENS, n(5))]);
        assert_eq!(w.store.get(w.total), n(5));

        let outcome = log.undo_one(open);
        assert_eq!(applied(&outcome)[0].changes.len(), 3);
        let mut resolve = |k: &String| slot(&w, k);
        apply_prev(&w.store, &mut resolve, applied(&outcome));
        assert_eq!(w.store.get(slot(&w, TOKENS)), n(0));

        let outcome = log.redo_one();
        let mut resolve = |k: &String| slot(&w, k);
        apply_next(&w.store, &mut resolve, applied(&outcome));
        assert_eq!(w.store.get(slot(&w, TOKENS)), n(5));
    }
}
