//! 回放引擎：把 journal 里的 [`Record`] 按写入顺序喂给一份
//! [`agent_store::persist::SessionLog`]。跟 `crate::jsonl::load` 是同一个算法（红线：
//! `Memory`/`Jsonl`/这里必须共用同一套「游标怎么翻译、snapshot 怎么压实」的推导，
//! 见 `agent_store::persist::log` 模块文档），只是「从文件读一行」换成了「按前缀扫
//! 一个 key」——这正是 114a 要求的「回放语义与 IndexedDB 绑定分开」：这个文件不认识
//! `KvStore` 的哪个实现在供数据，`MemoryKv`（native 测试）和 `web_kv`（真的浏览器）
//! 走的是完全同一份重放代码。
//!
//! 三个入口：
//! - [`replay_all`]：一份全新的 `SessionLog` + 扫到的记录条数，[`load_async`] 和
//!   `worker.rs` 起步「追平已有数据」共用同一份逻辑。
//! - [`seed`]：`worker.rs` 专用——起步失败一律静默退化成空 journal（理由见其文档）。
//! - [`load_async`]：转成 `SessionStore::load` 要的 `LoadOutcome` 三态。
//!
//! ## 跟 `Jsonl` 不一样的地方：没有「尾部半行」这一态
//!
//! `Jsonl` 要区分「最后一行没写完」（容忍，从这里截断）和「中间一行损坏」（拒绝）
//! ——那是 append-only *文件* 特有的「写到一半断电」语义。`KvStore::put` 要么整个
//! key 落定要么没落定，不存在「半个 value」这回事（见 `error.rs` 模块文档），所以
//! 这里每条记录反序列化失败都当同一类处理：整份拒绝、不吞、经 `on_error` 上报。

use serde::de::DeserializeOwned;

use agent_store::persist::{LoadOutcome, SessionLog};

use super::error::IdbStoreError;
use super::kv::KvStore;
use super::record::{Record, apply, journal_prefix};

/// 扫描整个 journal，按 [`KvStore::scan_prefix`] 返回的顺序（== 写入序，见
/// `record.rs`）重放进一份全新的 `SessionLog`。返回值第二个字段是「扫到了多少条
/// journal 记录」——`worker.rs` 起步时用它把 `next_index` 追平到已有内容，不需要为
/// 同一件事再扫一遍（一次 `scan_prefix` 两处用）。
pub(super) async fn replay_all<K, V, M>(
    kv: &impl KvStore,
) -> Result<(SessionLog<K, V, M>, u64), IdbStoreError>
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    let rows = kv
        .scan_prefix(journal_prefix())
        .await
        .map_err(IdbStoreError::Kv)?;

    let mut log: SessionLog<K, V, M> = SessionLog::new();
    for (index, (_key, value)) in rows.iter().enumerate() {
        let record: Record<K, V, M> =
            serde_json::from_slice(value).map_err(|_| IdbStoreError::CorruptRecord { index })?;
        apply(&mut log, record);
    }
    Ok((log, rows.len() as u64))
}

/// `worker.rs` 起步「追平已有数据」专用：任何失败都静默退化成一份空 journal（索引
/// 从 0 起步）——真正的错误报告属于应用层显式调用 `load()` 的那一次，不是这里，跟
/// `crate::jsonl::load::seed_from_disk` 同一条理由（见该函数文档「`seed_from_disk`」
/// 一节）：如果这里也报错，`on_error` 会在进程/工作线程刚起步、用户还没做任何操作
/// 时就响一次，而应用层显式 `load()`/`recover()` 时还会因为同一个原因再报一次
/// ——两次报告说的是同一件事，只会让人以为出了两个问题。
pub(super) async fn seed<K, V, M>(kv: &impl KvStore) -> (SessionLog<K, V, M>, u64)
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    replay_all(kv)
        .await
        .unwrap_or_else(|_| (SessionLog::new(), 0))
}

/// [`agent_store::SessionStore::load`] 的产物：重放全部 journal，三态化（`Absent`/
/// `Refused`/`Loaded`，见 [`LoadOutcome`] 文档）。
pub(super) async fn load_async<K, V, M>(
    kv: &impl KvStore,
    on_error: &(dyn Fn(IdbStoreError) + Send + Sync),
) -> LoadOutcome<K, V, M>
where
    K: Clone + DeserializeOwned,
    V: Clone + DeserializeOwned,
    M: Clone + DeserializeOwned,
{
    match replay_all(kv).await {
        Ok((log, _count)) => log
            .to_loaded()
            .map_or(LoadOutcome::Absent, LoadOutcome::Loaded),
        Err(e) => {
            on_error(e.clone());
            LoadOutcome::Refused {
                reason: e.to_string(),
            }
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::persist::idb::blocking::run_to_completion;
    use crate::persist::idb::memory_kv::MemoryKv;
    use crate::persist::idb::record::journal_key;
    use agent_store::history::{Change, Entry, Snapshot};

    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
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

    fn put_record(kv: &MemoryKv, index: u64, record: &Record<String, V, u32>) {
        let bytes = serde_json::to_vec(record).unwrap();
        run_to_completion(kv.put(&journal_key(index), &bytes)).unwrap();
    }

    #[test]
    fn an_empty_journal_replays_to_nothing() {
        let kv = MemoryKv::new();
        let (log, count) = run_to_completion(replay_all::<String, V, u32>(&kv)).unwrap();
        assert!(log.to_loaded().is_none());
        assert_eq!(count, 0);
    }

    #[test]
    fn entries_replay_in_scan_order_which_is_write_order() {
        let kv = MemoryKv::new();
        for i in 0..5u64 {
            put_record(&kv, i, &Record::Entry(entry(i)));
        }
        let (log, count) = run_to_completion(replay_all::<String, V, u32>(&kv)).unwrap();
        assert_eq!(count, 5);
        let loaded = log.to_loaded().unwrap();
        assert_eq!(
            loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
    }

    #[test]
    fn a_snapshot_record_compacts_exactly_like_jsonl_does() {
        let kv = MemoryKv::new();
        put_record(&kv, 0, &Record::Entry(entry(0)));
        put_record(
            &kv,
            1,
            &Record::Snapshot(Snapshot {
                values: vec![("a".to_string(), V(9))],
            }),
        );
        let (log, _) = run_to_completion(replay_all::<String, V, u32>(&kv)).unwrap();
        let loaded = log.to_loaded().unwrap();
        assert!(loaded.entries.is_empty(), "被这一张快照整个压实");
        assert_eq!(loaded.next_seq, 1, "但 seq 高水位没有跌回 0");
    }

    #[test]
    fn a_corrupt_record_refuses_the_whole_load_not_just_that_one() {
        let kv = MemoryKv::new();
        run_to_completion(kv.put(&journal_key(0), b"not json")).unwrap();
        let err = match run_to_completion(replay_all::<String, V, u32>(&kv)) {
            Err(e) => e,
            Ok(_) => panic!("损坏的记录不该让 replay_all 成功"),
        };
        assert_eq!(err, IdbStoreError::CorruptRecord { index: 0 });
    }

    #[test]
    fn seed_falls_back_to_an_empty_log_on_any_failure_without_reporting() {
        let kv = MemoryKv::new();
        run_to_completion(kv.put(&journal_key(0), b"not json")).unwrap();
        let (log, next_index) = run_to_completion(seed::<String, V, u32>(&kv));
        assert!(log.to_loaded().is_none());
        assert_eq!(next_index, 0);
    }

    #[test]
    fn load_async_reports_the_error_and_refuses() {
        let kv = MemoryKv::new();
        run_to_completion(kv.put(&journal_key(0), b"not json")).unwrap();
        let reported = std::sync::Mutex::new(Vec::new());
        let outcome = run_to_completion(load_async::<String, V, u32>(&kv, &|e| {
            reported.lock().unwrap().push(e);
        }));
        assert!(outcome.is_refused());
        assert_eq!(reported.lock().unwrap().len(), 1);
    }
}
