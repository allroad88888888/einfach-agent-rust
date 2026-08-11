//! 专用 IO 线程：真正跟 [`KvStore`] 打交道的地方（native only——理由见
//! `blocking.rs` 模块文档，这里不重复）。跟 `crate::jsonl::io_thread` 是同一个手法
//! （issue 011 硬约束：写扔给这个线程，actor 不等）、同一套「mirror 连续存活，落盘
//! 前用它已经算好的净效果」的推导（`crate::jsonl::io_thread` 模块文档那几节原样
//! 适用，这里不重复），换的只是「写一行」→「`put` 一个 journal key」。
//!
//! ## 起步：`seed_from_disk` 换成 [`replay::seed`]（同一个理由）
//!
//! 新起的工作线程如果不知道 KV 里已经有多少条 journal 记录，`next_index` 会从 0
//! 重开，后续 `put` 会**覆盖**已经存在的 journal key（同一个 index 被写两次）——
//! 不是「重复计数」这么轻，是把旧记录物理冲掉。起步先 [`replay::seed`] 把 mirror
//! 追平、把 `next_index` 设成「已经有多少条」，新写入才会接在后面而不是盖上去
//! ——这正是 `crate::jsonl::io_thread` 模块文档记的那个真 bug 在这个后端的对应修法。
//!
//! ## 写失败之后：跟 `Jsonl` 刻意不同的一点
//!
//! `Jsonl` 打不开文件之后「报一次，之后静默吞」——文件打不开的根因（权限、坏路径）
//! 几乎必然对下一次写入仍然成立，重试没有意义。KV 的一次 `put` 失败没有这么强的
//! 保证（配额、事务冲突这类原因可能是瞬时的），所以这里选择**每次失败都报**、且
//! **不推进 `next_index`**——mirror 已经记账过了（fire-and-forget 明确接受的风险，
//! 跟 `Jsonl` 一致），但下一条消息落盘时会重新尝试同一个 index，不会因为一次失败
//! 就在 journal 里留一个永久的空洞。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use serde::Serialize;
use serde::de::DeserializeOwned;

use agent_store::history::{Entry, Snapshot};
use agent_store::persist::SessionLog;

use super::blocking::run_to_completion;
use super::error::IdbStoreError;
use super::kv::KvStore;
use super::record::{Record, journal_key};
use super::replay;

pub(super) enum Msg<K, V, M> {
    Append(Entry<K, V, M>),
    DropOldest(usize),
    DropAfter { first_seq: u64, count: usize },
    SetCursor(usize),
    Snapshot(Snapshot<K, V>),
    /// 排干信号：处理到这条消息时，前面的写入必然都已经真正 `put` 完（`mpsc` 是
    /// FIFO）——`flush()`/`load()` 靠这个手法确认「不只是入队，是真的写完了」，跟
    /// `crate::jsonl::io_thread::Msg::Flush` 同一个手法。
    Flush(Sender<()>),
}

type OnError = Arc<dyn Fn(IdbStoreError) + Send + Sync>;

pub(super) fn run<K, V, M, KV>(kv: Arc<KV>, rx: Receiver<Msg<K, V, M>>, on_error: OnError)
where
    K: Clone + Serialize + DeserializeOwned,
    V: Clone + Serialize + DeserializeOwned,
    M: Clone + Serialize + DeserializeOwned,
    KV: KvStore,
{
    let (mut mirror, mut next_index): (SessionLog<K, V, M>, u64) =
        run_to_completion(replay::seed(kv.as_ref()));

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Append(entry) => {
                mirror.record_append(&entry);
                write(
                    kv.as_ref(),
                    &mut next_index,
                    Record::Entry(entry),
                    &on_error,
                );
            }
            Msg::DropOldest(count) => {
                let removed = mirror.record_drop_oldest(count);
                write::<K, V, M, KV>(
                    kv.as_ref(),
                    &mut next_index,
                    Record::DropOldest { count: removed },
                    &on_error,
                );
            }
            Msg::DropAfter { first_seq, count } => {
                mirror.record_drop_after(first_seq, count);
                write::<K, V, M, KV>(
                    kv.as_ref(),
                    &mut next_index,
                    Record::DropAfter { first_seq, count },
                    &on_error,
                );
            }
            Msg::SetCursor(cursor) => {
                mirror.record_cursor(cursor);
                // 落盘的是换算之后的相对游标，不是调用方给的原始 cursor——见
                // `crate::jsonl::io_thread` 模块文档「压实之后为什么不能落原始值」，
                // 这里的推导逐字适用。
                let cursor = mirror.relative_cursor();
                write::<K, V, M, KV>(
                    kv.as_ref(),
                    &mut next_index,
                    Record::Cursor { cursor },
                    &on_error,
                );
            }
            Msg::Snapshot(snap) => {
                mirror.record_snapshot(&snap);
                write::<K, V, M, KV>(
                    kv.as_ref(),
                    &mut next_index,
                    Record::Snapshot(snap),
                    &on_error,
                );
            }
            Msg::Flush(ack) => {
                let _ = ack.send(());
            }
        }
    }
}

/// 序列化一条记录、`put` 到下一个 journal key。成功才推进 `next_index`——见模块
/// 文档「写失败之后」一节。
fn write<K: Serialize, V: Serialize, M: Serialize, KV: KvStore>(
    kv: &KV,
    next_index: &mut u64,
    record: Record<K, V, M>,
    on_error: &OnError,
) {
    // 序列化失败不该发生（红线 3：primitive 必须全部可序列化），这里只做防御，
    // 不当成「KV 坏了」处理——不吞掉这条消息对应的记账（`mirror` 已经更新过了），
    // 只是这一条没能落盘；跟 `crate::jsonl::io_thread::write_line` 同一条取舍。
    let Ok(bytes) = serde_json::to_vec(&record) else {
        return;
    };
    let key = journal_key(*next_index);
    match run_to_completion(kv.put(&key, &bytes)) {
        Ok(()) => *next_index += 1,
        Err(e) => on_error(IdbStoreError::Kv(e)),
    }
}
