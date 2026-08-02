//! 采集与灌回：`Store` 与 [`Snapshot`] 之间的整份搬运（两个方向，一件事）。这里认
//! 进程内的 `AtomId`，可落盘的那一侧（[`Snapshot`] 本身）在同目录的
//! [`snapshot`](super::snapshot)，那个文件里没有 `AtomId` 这个符号 —— 红线 4。
//!
//! ## 键 → atom 的枚举由上层给
//!
//! 「哪些槽位属于这个会话」是 family 遍历，而 family 的键的语义（`AtomKey`：哪个
//! agent、哪个 slot）store 层不知道也不该知道。所以 [`capture`] 收一个迭代器而不是
//! 自己去翻 store：翻 store 只能拿到 `AtomId`，而按 id 存盘正是红线 4 禁的那件事。
//!
//! ## 恢复不是一个 undo 步
//!
//! [`restore`] 不产出 [`Change`](super::Change)、不碰 [`History`](super::History)：
//! 它铺的是**世界的起点**，不是世界里的一步。快照点之后的那些步由
//! [`apply_next`](super::apply_next) 一路往前推 —— 恢复就是 redo，同一个函数，不写第二套
//! 加载逻辑（`docs/STATE-MODEL.md` §「恢复 = redo」）。

use crate::ids::AtomId;
use crate::store::{AtomValue, Store};

use super::snapshot::Snapshot;

/// 采集：把上层给的「逻辑键 → atom」枚举逐个读成 `(键, 值)`。
///
/// **只喂 primitive。** derived 喂进来记下的是当时算出来的值，恢复时它会被下游重算
/// 覆盖 —— 白存一份，还在 schema 演进时变成对不上的一致性负担（见 [`Snapshot`]）。
///
/// 顺序原样保留，不排序、不去重：喂进来什么顺序就是什么顺序（理由见 [`Snapshot`]）。
/// 值是 `store.get` 的 owned 克隆，大值必须 `Arc` 包住（红线 5），否则整份采集就是一次
/// 深拷贝。
pub fn capture<K, V: AtomValue>(
    store: &Store<V>,
    atoms: impl Iterator<Item = (K, AtomId)>,
) -> Snapshot<K, V> {
    Snapshot {
        values: atoms.map(|(key, id)| (key, store.get(id))).collect(),
    }
}

/// 灌回：对每个 `(k, v)` 走 `resolve` 拿到当前图里的 atom，再 `set`。
///
/// # 三种键差异，三种答案
///
/// - **两边都有**：写回去，就这一件事。
/// - **快照里没有**（这一版新增的槽位）：这里什么都不做 —— 构图函数建它时给的默认值
///   就是答案。所以「往构图函数中间插一个新 atom」不需要迁移脚本。
/// - **快照里多出来**（这一版删掉的槽位）：`resolve` 返回 `None`，转 `on_unknown`。
///
/// # 为什么 `resolve` 返回 `Option`，而 applier 的是 get-or-create
///
/// **这是本模块唯一一处与 [`apply_prev`](super::apply_prev) / [`apply_next`](super::apply_next)
/// 分岔的地方，分岔有理由**：applier 的键来自本进程刚写出来的日志，一定属于当前 schema，
/// 「不在」只可能是被逐出了（019），get-or-create 建回来正是对的。而 `restore` 的键来自
/// **上一次进程的 schema**，可能是这版代码里已经不存在的槽位 —— 对它 get-or-create 会
/// 凭空造出一个没人读、也永远不会被回收的 atom（泄漏，且状态里多出一个不属于当前 schema
/// 的槽位）。所以这里必须能说「不认识」。
///
/// `on_unknown` 而不是静默丢：多出来的键意味着上一版的数据在这一版无家可归，上层要能
/// 记一条 warn（issue 010 原文）。日志/告警是 IO，store 层不做（红线 7），只给回调。
///
/// # 为什么整批包在一个 `batch` 里
///
/// 恢复是一次状态跃迁，中间态不该被任何 derived 看见。不批就是每写一个 primitive 冲一次
/// flush：下游 derived 在「一半上一次会话、一半这一次会话」的世界上重算若干次，而且那时
/// 后面的槽位可能还没被写。批到最后一次 flush，下游只重算一次，且在全部值就位之后 ——
/// 和 applier 那一批是同一个理由（019）。
pub fn restore<K, V: AtomValue>(
    store: &Store<V>,
    resolve: &mut impl FnMut(&K) -> Option<AtomId>,
    snap: &Snapshot<K, V>,
    on_unknown: &mut impl FnMut(&K),
) {
    store.batch(|s| {
        for (key, value) in &snap.values {
            match resolve(key) {
                Some(id) => s.set(id, value.clone()),
                None => on_unknown(key),
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

    #[derive(Clone, Debug, PartialEq)]
    struct Cell(i64);

    impl AtomValue for Cell {
        fn null() -> Self {
            Cell(0)
        }
    }

    /// 上层 Slot 表在测试里的最小形状：默认值由**键**决定。`mid` 的默认值特意不是 0，
    /// 好让「新增的槽位取默认值」是可观测的。
    fn default_for(key: &str) -> Cell {
        match key {
            "mid" => Cell(7),
            _ => Cell::null(),
        }
    }

    struct World {
        store: Store<Cell>,
        fam: Rc<RefCell<AtomFamily<String>>>,
        sum: AtomId,
    }

    /// 唯一的创建路径：写槽位、`resolve` 找槽位、derived 现查槽位走的都是它。
    fn slot(w: &World, key: &str) -> AtomId {
        w.fam
            .borrow_mut()
            .get_or_create(key.to_string(), || w.store.create_atom(default_for(key)))
    }

    /// `keys` 里所有槽位之和的 derived，**按逻辑键现查 family**（红线 4 的孪生条款：
    /// derived 闭包里不许焊 `AtomId`）。
    fn build(keys: &'static [&'static str]) -> World {
        let store: Store<Cell> = Store::new();
        let fam: Rc<RefCell<AtomFamily<String>>> = Rc::new(RefCell::new(AtomFamily::new()));
        let (st, fm) = (store.clone(), fam.clone());
        let sum = store.create_derived_ctx(move |args| {
            let mut acc = 0;
            for key in keys {
                let id = fm
                    .borrow_mut()
                    .get_or_create((*key).to_string(), || st.create_atom(default_for(key)));
                acc += args.get(id).0;
            }
            Cell(acc)
        });
        let w = World { store, fam, sum };
        let _ = w.store.get(w.sum); // 建立反向依赖边
        w
    }

    fn snap(pairs: &[(&str, i64)]) -> Snapshot<String, Cell> {
        Snapshot {
            values: pairs.iter().map(|(k, v)| ((*k).to_string(), Cell(*v))).collect(),
        }
    }

    /// 灌回一份快照，返回被判为「不认识」的键。`resolve` 是**非创建**查找。
    fn restore_into(w: &World, s: &Snapshot<String, Cell>) -> Vec<String> {
        let mut unknown = Vec::new();
        let mut resolve = |k: &String| w.fam.borrow().get(k);
        restore(&w.store, &mut resolve, s, &mut |k: &String| {
            unknown.push(k.clone())
        });
        unknown
    }

    #[test]
    fn capture_reads_the_live_value_of_every_atom_it_is_handed_in_the_order_given() {
        let w = build(&["a", "b"]);
        w.store.set(slot(&w, "a"), Cell(5));
        w.store.set(slot(&w, "b"), Cell(6));

        let s = capture(
            &w.store,
            [("b".to_string(), slot(&w, "b")), ("a".to_string(), slot(&w, "a"))].into_iter(),
        );
        assert_eq!(s.values, vec![("b".into(), Cell(6)), ("a".into(), Cell(5))]);
    }

    #[test]
    fn an_empty_iterator_captures_nothing_and_an_empty_snapshot_restores_nothing() {
        let w = build(&["a", "b"]);
        w.store.set(slot(&w, "a"), Cell(5));
        assert!(capture(&w.store, std::iter::empty::<(String, AtomId)>()).values.is_empty());

        let before = w.store.debug_recompute_count();
        assert!(restore_into(&w, &snap(&[])).is_empty());
        assert_eq!(w.store.get(w.sum), Cell(5)); // 世界没动
        assert_eq!(w.store.debug_recompute_count(), before); // 空批次不重算
    }

    #[test]
    fn restore_lands_every_known_key_and_the_downstream_recomputes_exactly_once() {
        let w = build(&["a", "b"]);
        let _ = slot(&w, "a");
        let _ = slot(&w, "b");
        let before = w.store.debug_recompute_count();

        assert!(restore_into(&w, &snap(&[("a", 10), ("b", 20)])).is_empty());
        // 整批一个 batch：下游只在全部值就位之后重算一次，看不到「一半旧一半新」。
        assert_eq!(w.store.debug_recompute_count() - before, 1);
        assert_eq!(w.store.get(slot(&w, "a")), Cell(10));
        assert_eq!(w.store.get(w.sum), Cell(30));
    }

    #[test]
    fn a_key_this_version_no_longer_has_is_reported_and_the_rest_still_land() {
        // 「删掉的 slot」：旧快照里多出来的键 → on_unknown 收到，其余照常灌回。
        let w = build(&["a", "b"]);
        let unknown = restore_into(&w, &snap(&[("a", 10), ("dropped_slot", 99), ("b", 20)]));

        assert_eq!(unknown, vec!["dropped_slot".to_string()]);
        assert_eq!(w.store.get(w.sum), Cell(30));
        // 不认识的键没有在图里凭空造出一个 atom —— resolve 是非创建查找。
        assert!(w.fam.borrow().get(&"dropped_slot".to_string()).is_none());
    }

    #[test]
    fn a_key_absent_from_the_snapshot_keeps_the_default_the_build_function_gave_it() {
        // 「新增的 slot」：旧快照里找不到 mid，构图函数给的 7 就是答案，restore 不碰。
        let w = build(&["a", "mid"]);
        assert_eq!(w.store.get(w.sum), Cell(7));

        assert!(restore_into(&w, &snap(&[("a", 10)])).is_empty());
        assert_eq!(w.store.get(slot(&w, "mid")), Cell(7));
        assert_eq!(w.store.get(w.sum), Cell(17));
    }

    #[test]
    fn a_duplicated_key_is_last_write_wins() {
        // 去重要 Hash/Ord，快照层不做（见 `Snapshot`）。行为可预测就够：后写覆盖先写。
        let w = build(&["a", "b"]);
        assert!(restore_into(&w, &snap(&[("a", 1), ("a", 2)])).is_empty());
        assert_eq!(w.store.get(slot(&w, "a")), Cell(2));
    }
}
