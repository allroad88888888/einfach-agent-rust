//! `SessionStore` 的内存实现：测试与临时会话用，进程退出即丢。
//!
//! 纯粹是 [`SessionLog`] 外面套一层 `Mutex`——`SessionStore` 的方法都是 `&self`
//! （actor 单线程调用，但类型本身仍需要 `Sync` 才能塞进 `Arc<dyn SessionStore<..>>`），
//! 记账逻辑全部委托给引擎，这个文件不重新推一遍游标怎么翻译。

use std::sync::Mutex;

use crate::history::{Entry, Snapshot};

use super::log::SessionLog;
use super::{LoadOutcome, SessionStore};

pub struct Memory<K, V, M> {
    inner: Mutex<SessionLog<K, V, M>>,
}

impl<K, V, M> Memory<K, V, M> {
    pub fn new() -> Self {
        Memory {
            inner: Mutex::new(SessionLog::new()),
        }
    }
}

impl<K, V, M> Default for Memory<K, V, M> {
    fn default() -> Self {
        Self::new()
    }
}

/// 锁中毒（某次持锁时 panic）按「这个后端从今往后再也读不到／写不进任何东西」处理，
/// 而不是把 panic 传染给调用方——那正是 `Memory` 存在的意义之一：它是给测试用的
/// 无 IO 后端，唯一可能失败的地方就是自己的锁，不该比 `Jsonl` 更容易把上层带崩。
fn with_lock<K, V, M, R>(
    inner: &Mutex<SessionLog<K, V, M>>,
    f: impl FnOnce(&mut SessionLog<K, V, M>) -> R,
    default: R,
) -> R {
    match inner.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(_) => default,
    }
}

impl<K: Clone, V: Clone, M: Clone> SessionStore<K, V, M> for Memory<K, V, M> {
    fn append(&self, entry: &Entry<K, V, M>) {
        with_lock(&self.inner, |log| log.record_append(entry), ());
    }

    fn drop_oldest(&self, count: usize) {
        // 返回值（真正切掉了多少条）只有 `Jsonl` 落盘需要——见 `record_drop_oldest`
        // 文档；`Memory` 是单一份连续存活的 `SessionLog`，不存在「从压实点重放」这回事，
        // 用不上它。
        with_lock(
            &self.inner,
            |log| {
                log.record_drop_oldest(count);
            },
            (),
        );
    }

    fn drop_after(&self, first_seq: u64, count: usize) {
        with_lock(
            &self.inner,
            |log| log.record_drop_after(first_seq, count),
            (),
        );
    }

    fn set_cursor(&self, cursor: usize) {
        with_lock(&self.inner, |log| log.record_cursor(cursor), ());
    }

    fn snapshot(&self, snap: &Snapshot<K, V>) {
        with_lock(&self.inner, |log| log.record_snapshot(snap), ());
    }

    fn load(&self) -> LoadOutcome<K, V, M> {
        // `Memory` 没有序列化步骤（红线 7：这个模块零 IO），天生不会 `Refused`
        // ——`to_loaded()` 只有「从没写过」与「有数据」两种可能。
        with_lock(
            &self.inner,
            |log| {
                log.to_loaded()
                    .map_or(LoadOutcome::Absent, LoadOutcome::Loaded)
            },
            LoadOutcome::Absent,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Change;

    #[derive(Clone, Debug, PartialEq)]
    struct V(i64);

    fn entry(seq: u64) -> Entry<String, V, u32> {
        Entry {
            seq,
            meta: 1,
            changes: vec![Change {
                key: "a".to_string(),
                prev: V(seq as i64),
                next: V(seq as i64 + 1),
            }],
        }
    }

    #[test]
    fn a_fresh_store_has_nothing_to_load() {
        let store: Memory<String, V, u32> = Memory::new();
        assert!(store.load().is_absent());
    }

    #[test]
    fn append_then_set_cursor_then_load_round_trips() {
        let store: Memory<String, V, u32> = Memory::new();
        for i in 0..3 {
            store.append(&entry(i));
        }
        store.set_cursor(3);

        let loaded = store.load().loaded().unwrap();
        assert_eq!(
            loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        assert_eq!(loaded.cursor, 3);
        assert_eq!(loaded.next_seq, 3);
        assert!(loaded.snapshot.is_none());
    }

    /// `Memory` 零 IO（红线 7）、没有序列化步骤——`load()` 天生不会给出 `Refused`，
    /// 那一态只在 `Jsonl`（agent-runtime，真的解析字节）一侧才可能出现。
    #[test]
    fn memory_never_refuses_a_load() {
        let store: Memory<String, V, u32> = Memory::new();
        store.append(&entry(0));
        assert!(!store.load().is_refused());
    }

    /// `SessionStore` 是共享调用点（`&self`），测试它真的能塞进 `Arc<dyn ..>` 被多方
    /// 持有——`Sync` 是这类用法编译期就会拦下的东西，这里显式验一次而不是等调用方踩到。
    #[test]
    fn is_usable_behind_an_arc_dyn_session_store() {
        let store: std::sync::Arc<dyn SessionStore<String, V, u32>> =
            std::sync::Arc::new(Memory::<String, V, u32>::new());
        store.append(&entry(0));
        store.set_cursor(1);
        assert_eq!(store.load().loaded().unwrap().entries.len(), 1);
    }
}
