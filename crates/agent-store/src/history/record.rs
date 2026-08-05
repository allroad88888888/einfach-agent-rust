//! 记录入口：把一次 store 写入变成一条 [`Change`]。
//!
//! 这是红线 2「写入必须收口」的机制面。**显式声明是唯一可行解**：自动捕获变更要给
//! 每个被追踪的 atom 常驻订阅和基线值，成本 O(被追踪 atom 数) —— 本仓每个 agent 的
//! 每个槽位都是 family atom，子 agent 还是动态增长的，这个成本不成立（上游 TS 的
//! `createHistory` 踩过并写进了注释）。
//!
//! 「derived 不产生 `Entry`」因此不是纪律而是结构：derived 的值是 store 内部
//! `flush_pending` → `dependencies_change` 重算出来的，那条路径根本不经过这个文件。
//! 这里唯一能记的就是调用方显式点名的那个 atom。

use crate::ids::AtomId;
use crate::store::{AtomValue, Store};

use super::log::Change;

/// 记录一次 primitive 写入：capture prev → `store.set` → 产出一条 [`Change`]。
///
/// **这是「写入必须收口」的唯一合法通道**（红线 2）。业务代码不许直接 `store.set`：
/// 绕过去的那次写入不进 undo log，undo 越过它时这个 atom 停在新值上、其余全部回滚,
/// 状态自相矛盾 —— 而且是「测试全过、线上偶发」的那种矛盾。
///
/// `prev` 是**写入前当场读**的（`store.get(atom)`，primitive 首读会从 init 落值），
/// 不是事后从日志推算的。`key` 是上层选的逻辑键，`atom` 只是本进程里找到那个槽位的
/// 句柄，不会进日志（红线 4）。
///
/// 返回 `None` 有且只有一种情况：`prev == next`（`PartialEq` 相等）。不变的写入不进
/// 日志 —— 它的逆操作是空操作，进了日志就是一个 undo 时「按一下没反应」的幽灵步。
/// 这一跳过对 primitive 是不可观测的：store 自己的 `set_atom_state` 第一件事就是同一个
/// `PartialEq` 比较，相等即提前返回，不落 pending、不传播。
///
/// # 一次 batch = 一个 undo 步
///
/// 事务边界直接复用 `store.batch`，不另造概念。一次 batch 里多个 `record_set` 产出的
/// `Change` 由**调用方**攒成一个 `Vec` 喂给 [`History::append`](super::History::append)：
///
/// ```
/// # use agent_store::{AtomValue, Store};
/// # use agent_store::history::{record_set, Change, History};
/// # #[derive(Clone, Debug, PartialEq)]
/// # struct V(i64);
/// # impl AtomValue for V { fn null() -> Self { V(0) } }
/// let store: Store<V> = Store::new();
/// let (a, b) = (store.create_atom(V(1)), store.create_atom(V(2)));
/// let mut history: History<String, V, &'static str> = History::new();
///
/// let mut changes: Vec<Change<String, V>> = Vec::new();
/// store.batch(|s| {
///     changes.extend(record_set(s, "a".to_string(), a, V(10)));
///     changes.extend(record_set(s, "b".to_string(), b, V(20)));
/// });
/// // 两处变更 → 一条 entry → undo 一下同时退回两处。
/// assert_eq!(history.append("two_writes", changes), Some(0));
/// assert_eq!(history.last().unwrap().changes.len(), 2);
/// ```
///
/// 只写 primitive。传一个 derived atom 进来，`prev`/`next` 记的是算出来的值，而那不是
/// 一个可回放的逆操作 —— 恢复时重算的是它的依赖，不是它自己。
///
/// # 为什么 `#[must_use]`
///
/// 丢弃返回值 = 值写进了 store 却没进日志，正是红线 2 要挡的那个洞：undo 越过这一步时
/// 这个 atom 停在新值上、其余全部回滚。而它长得像一句普通的写入语句，`record_set(..);`
/// 一行不会有任何报错 —— 编译器是唯一能在这里出声的人。
/// （009 挂着这笔账：当时有并行的独立测试 agent 按字面签名在写验收，加 `#[must_use]`
/// 会在 `-D warnings` 下炸掉他们的构建。017 补上。）
#[must_use = "record_set 的返回值就是这次写入的 undo 记录，丢掉它 = 这一步不可回滚"]
pub fn record_set<V: AtomValue, K>(
    store: &Store<V>,
    key: K,
    atom: AtomId,
    next: V,
) -> Option<Change<K, V>> {
    let prev = store.get(atom);
    if prev == next {
        return None;
    }
    store.set(atom, next.clone());
    Some(Change { key, prev, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;

    /// 最小的 `AtomValue`（015 的 TestValue 手法，只留这里用得着的两支）。
    /// 刻意**不** derive `Serialize`：这个文件里有 `AtomId`，红线 4 的 grep 检查的
    /// 正是「同一文件里既有 Serialize 派生又出现 AtomId」。序列化的测试在 `log.rs`。
    #[derive(Clone, Debug, PartialEq)]
    enum Tv {
        Num(i64),
        Text(&'static str),
    }

    impl AtomValue for Tv {
        fn null() -> Self {
            Tv::Num(0)
        }
    }

    fn num(n: i64) -> Tv {
        Tv::Num(n)
    }

    type Log = History<String, Tv, &'static str>;

    #[test]
    fn captures_prev_before_the_write_and_next_after() {
        let store: Store<Tv> = Store::new();
        let a = store.create_atom(num(1));

        let change = record_set(&store, "a".to_string(), a, num(2)).unwrap();

        assert_eq!(change.key, "a");
        assert_eq!(change.prev, num(1)); // 写入前的值，来自 init
        assert_eq!(change.next, num(2));
        assert_eq!(store.get(a), num(2)); // 写真的落了
    }

    #[test]
    fn prev_is_the_live_value_not_the_init() {
        let store: Store<Tv> = Store::new();
        let a = store.create_atom(num(1));
        store.set(a, num(5));

        let change = record_set(&store, "a".to_string(), a, num(9)).unwrap();
        assert_eq!(change.prev, num(5));
    }

    #[test]
    fn unchanged_write_is_not_logged() {
        let store: Store<Tv> = Store::new();
        let a = store.create_atom(num(1));
        let mut history = Log::new();

        assert!(record_set(&store, "a".to_string(), a, num(1)).is_none());

        // 值没变 → 没有 change → 没有 entry。整条链上没有幽灵步。
        let changes: Vec<_> = record_set(&store, "a".to_string(), a, num(1))
            .into_iter()
            .collect();
        assert_eq!(history.append("noop", changes), None);
        assert!(history.is_empty());
        assert_eq!(store.get(a), num(1));
    }

    #[test]
    fn unchanged_is_by_partial_eq_not_by_variant() {
        let store: Store<Tv> = Store::new();
        let a = store.create_atom(Tv::Text("x"));
        assert!(record_set(&store, "a".to_string(), a, Tv::Text("x")).is_none());
        assert!(record_set(&store, "a".to_string(), a, Tv::Text("y")).is_some());
    }

    #[test]
    fn one_batch_of_two_writes_is_one_entry() {
        let store: Store<Tv> = Store::new();
        let (a, b) = (store.create_atom(num(1)), store.create_atom(num(2)));
        let mut history = Log::new();

        let mut changes = Vec::new();
        store.batch(|s| {
            changes.extend(record_set(s, "a".to_string(), a, num(10)));
            changes.extend(record_set(s, "b".to_string(), b, num(20)));
        });
        assert_eq!(history.append("two_writes", changes), Some(0));

        let entry = history.last().unwrap();
        assert_eq!(entry.changes.len(), 2);
        assert_eq!(entry.changes[0].key, "a");
        assert_eq!(entry.changes[1].key, "b");
    }

    #[test]
    fn two_writes_to_one_atom_in_a_batch_chain_prev_to_next() {
        // batch 里 set 只推迟 flush，不推迟落值 —— 第二次 record_set 读到的 prev 是
        // 第一次的 next，于是这条 entry 从头到尾仍然是可逆的。
        let store: Store<Tv> = Store::new();
        let a = store.create_atom(num(1));

        let mut changes = Vec::new();
        store.batch(|s| {
            changes.extend(record_set(s, "a".to_string(), a, num(2)));
            changes.extend(record_set(s, "a".to_string(), a, num(3)));
        });

        assert_eq!(changes.len(), 2);
        assert_eq!(
            (changes[0].prev.clone(), changes[0].next.clone()),
            (num(1), num(2))
        );
        assert_eq!(
            (changes[1].prev.clone(), changes[1].next.clone()),
            (num(2), num(3))
        );
        assert_eq!(store.get(a), num(3));
    }

    #[test]
    fn derived_recompute_produces_no_change() {
        // 「derived 不产生 Entry」的结构性证明：下游确实重算了（recompute 计数涨了、
        // 值变了），而日志里只有 primitive 那一条 —— 因为 derived 的值是 store 内部
        // flush 算出来的，那条路径不经过 record_set。
        let store: Store<Tv> = Store::new();
        let p = store.create_atom(num(1));
        let d = store.create_derived_ctx(move |args| match args.get(p) {
            Tv::Num(n) => Tv::Num(n * 2),
            other => other,
        });
        assert_eq!(store.get(d), num(2)); // 建立反向依赖边
        let recomputes_before = store.debug_recompute_count();

        let mut history = Log::new();
        let mut changes = Vec::new();
        store.batch(|s| {
            changes.extend(record_set(s, "p".to_string(), p, num(5)));
        });
        history.append("write_p", changes);

        assert_eq!(store.get(d), num(10));
        assert!(store.debug_recompute_count() > recomputes_before);

        assert_eq!(history.len(), 1);
        let entry = history.last().unwrap();
        assert_eq!(entry.changes.len(), 1);
        assert_eq!(entry.changes[0].key, "p");
        assert_eq!(entry.changes[0].next, num(5));
    }

    #[test]
    fn writable_derived_still_records_only_what_it_was_asked_to() {
        // writable derived 的 write fn 可以往任意多个 primitive 里写；那些写入不经过
        // record_set，所以**不会**进日志。这正是「唯一合法通道」的另一面：想让它们
        // 可回滚，就得由 command 层逐个 record_set，而不是指望 store 帮忙捕获。
        let store: Store<Tv> = Store::new();
        let backing = store.create_atom(num(0));
        let w = store.create_writable(
            move |args| args.get(backing),
            move |args, v| {
                let doubled = match v {
                    Tv::Num(n) => Tv::Num(n * 2),
                    other => other,
                };
                args.set(backing, doubled);
            },
        );

        let change = record_set(&store, "w".to_string(), w, num(3)).unwrap();
        assert_eq!(change.prev, num(0));
        assert_eq!(change.next, num(3)); // 记的是被要求写的值
        assert_eq!(store.get(backing), num(6)); // 真正被改的 primitive 无人记录
    }
}
