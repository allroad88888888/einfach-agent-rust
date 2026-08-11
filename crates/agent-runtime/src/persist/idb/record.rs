//! Journal 里一条记录长什么样，以及 key 怎么编。
//!
//! [`Record`] 跟 `crate::jsonl::record` 里那个私有的同名类型是同一个五变体 tagged
//! JSON——`SessionStore` 的五个写方法一一对应，没有第六种：回放
//! （[`apply`]/[`super::replay`]）按 `kind` 分发进
//! [`SessionLog`](agent_store::persist::SessionLog) 对应的 `record_*` 方法，写路径
//! （[`super::worker`]）反过来把每个 `SessionStore` 调用序列化成一条记录——两条路径
//! 的变体必须一一对应，否则会出现「写得出来、读不回去」的记录。
//!
//! ## key 怎么编：为什么是零填充十进制而不是原始 `u64` 大端字节
//!
//! [`KvStore::scan_prefix`](super::kv::KvStore::scan_prefix) 承诺按 key 的**字节**序
//! 返回；journal 要按写入顺序重放，字节序就必须等于写入序。
//! `format!("{JOURNAL_PREFIX}{index:020}")` 零填充到 20 位（`u64::MAX` 十进制正好
//! 20 位），字典序绝对不会因为「9」排在「10」前面而错——这是最直白、调试时肉眼就能
//! 认出顺序的编码。真正的 IndexedDB 实现（[`super::web_kv`]）如果想换成大端字节数组
//! （`to_be_bytes`）性能会更好，但那只是 key 的内部编码，不影响这个模块以外的任何
//! 人——不管用哪种编码，都必须满足「排序后等于写入序」这一条契约，`replay.rs` 只认
//! 这一条。

use serde::{Deserialize, Serialize};

use agent_store::history::{Entry, Snapshot};
use agent_store::persist::SessionLog;

pub(super) const JOURNAL_PREFIX: &str = "journal/";

/// 见模块文档：所有 journal 记录共用的前缀，[`super::replay`] 拿它去
/// [`KvStore::scan_prefix`](super::kv::KvStore::scan_prefix)。
pub(super) fn journal_prefix() -> &'static [u8] {
    JOURNAL_PREFIX.as_bytes()
}

/// 见模块文档：第 `index` 条 journal 记录的 key。
pub(super) fn journal_key(index: u64) -> Vec<u8> {
    format!("{JOURNAL_PREFIX}{index:020}").into_bytes()
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Record<K, V, M> {
    Entry(Entry<K, V, M>),
    Snapshot(Snapshot<K, V>),
    Cursor { cursor: usize },
    DropOldest { count: usize },
    DropAfter { first_seq: u64, count: usize },
}

/// 把一条记录喂给引擎——跟 `crate::jsonl::load::apply` 是同一份推导，两个后端各自
/// 复制一份而不是共享，是因为 `Record` 本身也是各自私有的类型（`jsonl` 那份
/// `pub(super)` 在 `jsonl` 模块内，这份 `pub(super)` 在 `idb` 模块内），真正共享的
/// 是它们共同依赖的 [`SessionLog`]。
pub(super) fn apply<K: Clone, V: Clone, M: Clone>(log: &mut SessionLog<K, V, M>, record: Record<K, V, M>) {
    match record {
        Record::Entry(e) => log.record_append(&e),
        Record::Snapshot(s) => log.record_snapshot(&s),
        Record::Cursor { cursor } => log.record_cursor(cursor),
        Record::DropOldest { count } => {
            log.record_drop_oldest(count);
        }
        Record::DropAfter { first_seq, count } => log.record_drop_after(first_seq, count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_keys_sort_lexicographically_in_write_order() {
        let indices = [0u64, 1, 9, 10, 99, 100, 999, 1000, 1_000_000];
        let keys: Vec<Vec<u8>> = indices.iter().map(|&i| journal_key(i)).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "零填充编码必须保证字节序 == 数值序，否则 scan_prefix 的重放顺序就是错的"
        );
    }

    #[test]
    fn every_journal_key_starts_with_the_scan_prefix() {
        assert!(journal_key(0).starts_with(journal_prefix()));
        assert!(journal_key(42).starts_with(journal_prefix()));
    }

    #[test]
    fn a_record_survives_a_json_round_trip() {
        let original: Record<String, i64, u32> = Record::Cursor { cursor: 3 };
        let bytes = serde_json::to_vec(&original).unwrap();
        let back: Record<String, i64, u32> = serde_json::from_slice(&bytes).unwrap();
        assert!(matches!(back, Record::Cursor { cursor: 3 }));
    }
}
