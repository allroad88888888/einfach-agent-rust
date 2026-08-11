//! [`IdbStore`]：`SessionStore` 装在一个通用 [`KvStore`] 上的 native 包装——工作
//! 线程 + channel，跟 `crate::Jsonl` 同一个形状（「你的实现是它的兄弟」），换的是
//! `File` → 任意 `KvStore` 实现。
//!
//! **只能在 native 编译**（`std::thread::spawn` 在 `wasm32-unknown-unknown` 上没有
//! 对应实现）——这是刻意的边界，不是疏漏：它存在的意义是「在没有浏览器的情况下
//! 证明 `KvStore` 泛型的回放/游标/压实引擎是对的」（114a 的验收主证据，用
//! [`super::memory_kv::MemoryKv`] 跑），不是给 wasm 生产环境用的最终形态。wasm
//! 那条路（114c）会重新组装 [`super::kv::KvStore`] + [`super::replay`]，派发换成
//! `wasm_bindgen_futures::spawn_local`——那条路径不需要、也用不了这里的
//! `std::thread`/`blocking.rs`。

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::Serialize;
use serde::de::DeserializeOwned;

use agent_store::SessionStore;
use agent_store::history::{Entry, Snapshot};
use agent_store::persist::LoadOutcome;

use super::blocking::run_to_completion;
use super::error::IdbStoreError;
use super::kv::KvStore;
use super::replay::load_async;
use super::worker::{self, Msg};

pub struct IdbStore<K, V, M, KV> {
    kv: Arc<KV>,
    tx: Mutex<Option<Sender<Msg<K, V, M>>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    on_error: Arc<dyn Fn(IdbStoreError) + Send + Sync>,
}

impl<K, V, M, KV> IdbStore<K, V, M, KV>
where
    K: Clone + Serialize + DeserializeOwned + Send + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + 'static,
    M: Clone + Serialize + DeserializeOwned + Send + 'static,
    KV: KvStore + Send + Sync + 'static,
{
    /// 起工作线程。跟 `Jsonl::new` 一样**从不失败**——KV 打不开/连不上的失败经
    /// `on_error` 报，不在构造这一步引入 `Result`（同一条理由：fire-and-forget 的
    /// 端口不该诱使调用方处理「万一失败怎么办」的分支）。
    pub fn spawn(kv: KV, on_error: impl Fn(IdbStoreError) + Send + Sync + 'static) -> Self {
        let kv = Arc::new(kv);
        let on_error: Arc<dyn Fn(IdbStoreError) + Send + Sync> = Arc::new(on_error);
        let (tx, rx) = mpsc::channel();
        let handle = {
            let kv = kv.clone();
            let on_error = on_error.clone();
            std::thread::spawn(move || worker::run(kv, rx, on_error))
        };
        IdbStore {
            kv,
            tx: Mutex::new(Some(tx)),
            handle: Mutex::new(Some(handle)),
            on_error,
        }
    }

    /// 排干队列：调用返回时，此前所有写方法产生的写入都已经真正处理完（落到 KV
    /// 或者确认放弃），不只是「已经入队」。`load()` 内部会先调它——跟
    /// `crate::Jsonl::flush` 同一个理由。
    pub fn flush(&self) {
        let (ack_tx, ack_rx) = mpsc::channel();
        let queued = self
            .tx
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|tx| tx.send(Msg::Flush(ack_tx)).is_ok()))
            .unwrap_or(false);
        if queued {
            let _ = ack_rx.recv();
        }
    }

    fn send(&self, msg: Msg<K, V, M>) {
        if let Ok(guard) = self.tx.lock()
            && let Some(tx) = guard.as_ref()
        {
            let _ = tx.send(msg); // 发送失败 = 工作线程已经不在了，静默丢弃
        }
    }
}

impl<K, V, M, KV> SessionStore<K, V, M> for IdbStore<K, V, M, KV>
where
    K: Clone + Serialize + DeserializeOwned + Send + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + 'static,
    M: Clone + Serialize + DeserializeOwned + Send + 'static,
    KV: KvStore + Send + Sync + 'static,
{
    fn append(&self, entry: &Entry<K, V, M>) {
        self.send(Msg::Append(entry.clone()));
    }

    fn drop_oldest(&self, count: usize) {
        self.send(Msg::DropOldest(count));
    }

    fn drop_after(&self, first_seq: u64, count: usize) {
        self.send(Msg::DropAfter { first_seq, count });
    }

    fn set_cursor(&self, cursor: usize) {
        self.send(Msg::SetCursor(cursor));
    }

    fn snapshot(&self, snap: &Snapshot<K, V>) {
        self.send(Msg::Snapshot(snap.clone()));
    }

    /// 例外（跟 `crate::Jsonl::load` 同一句话）：先 `flush()` 排干工作线程的队列，
    /// 再直接在调用线程上跑一遍完整重放——`self.kv` 是 `Arc`，工作线程和调用线程
    /// 共享同一个底层连接/句柄，重放看到的是 flush 之后的最新状态。
    fn load(&self) -> LoadOutcome<K, V, M> {
        self.flush();
        run_to_completion(load_async(self.kv.as_ref(), self.on_error.as_ref()))
    }
}

/// **排干时机**：先关发送端（`worker::run` 的 `recv()` 循环见到 channel 关闭才会
/// 退出，不然 `join` 会永远等一条不会来的消息），再 `join`——`join` 之前所有已入队
/// 的消息都会被工作线程处理完，这就是「drop 时排干」（issue 011 硬约束），跟
/// `crate::Jsonl::drop` 同一个手法。
impl<K, V, M, KV> Drop for IdbStore<K, V, M, KV> {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.tx.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.handle.lock()
            && let Some(h) = guard.take()
        {
            let _ = h.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persist::idb::memory_kv::MemoryKv;
    use agent_store::history::Change;

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

    #[test]
    fn a_fresh_store_has_nothing_to_load() {
        let store: IdbStore<String, V, u32, MemoryKv> = IdbStore::spawn(MemoryKv::new(), |_| {});
        assert!(store.load().is_absent());
    }

    #[test]
    fn append_then_set_cursor_then_load_round_trips() {
        let store: IdbStore<String, V, u32, MemoryKv> = IdbStore::spawn(MemoryKv::new(), |_| {});
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

    /// 重启场景（native 版本，`MemoryKv` 用浅克隆模拟「同一个数据库被重新连接」，
    /// 见 `memory_kv.rs` 模块文档）：drop 掉一个 `IdbStore`（排干队列）→ 用同一份
    /// 底层数据开第二个 `IdbStore` → 它的 `load()` 必须看到全部旧数据，新写入接
    /// 在旧 journal 后面而不是覆盖——这正是 `worker.rs` 模块文档「起步：
    /// `seed_from_disk` 换成 `replay::seed`」要防的那类 bug 的回归测试：`Jsonl`
    /// 那边就是因为工作线程起步时没追平已有内容，系统性地把游标写小，第三个重启
    /// 周期悄悄冲掉了上一轮真实写过的整轮对话（`crate::jsonl::load` 模块文档
    /// 「`seed_from_disk`」一节记的真 bug）。
    #[test]
    fn a_new_store_over_the_same_kv_seeds_its_next_index_from_existing_journal_entries() {
        let kv = MemoryKv::new();
        {
            let store: IdbStore<String, V, u32, MemoryKv> = IdbStore::spawn(kv.clone(), |_| {});
            for i in 0..3 {
                store.append(&entry(i));
            }
            store.set_cursor(3);
            store.flush();
            // `store` drop 在这里发生：`tx` 关闭 → 工作线程排干队列退出 → `join`。
        }

        let reopened: IdbStore<String, V, u32, MemoryKv> = IdbStore::spawn(kv, |_| {});
        let loaded_before_new_writes = reopened.load().loaded().unwrap();
        assert_eq!(
            loaded_before_new_writes
                .entries
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            vec![0, 1, 2],
            "重新连接同一份数据必须看到上一个 store 写过的全部历史"
        );

        // 接着写一条新的——如果 `next_index` 没有追平，这一条会落进 journal 的
        // index 0（覆盖第一条旧记录），下面的断言就会失败。
        reopened.append(&entry(3));
        reopened.set_cursor(4);
        let loaded = reopened.load().loaded().unwrap();
        assert_eq!(
            loaded.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2, 3],
            "新写入必须接在旧 journal 后面，不能覆盖任何一条"
        );
        assert_eq!(loaded.cursor, 4);
        assert_eq!(loaded.next_seq, 4);
    }
}
