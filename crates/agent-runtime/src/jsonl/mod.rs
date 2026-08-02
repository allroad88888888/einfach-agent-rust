//! `Jsonl`：`SessionStore`（issue 011）唯一做真文件 IO 的实现——`agent-core` 和
//! `agent-store` 都被红线 7 禁着，这个 crate 是运行时层，才轮到它碰 `std::fs`。
//!
//! ## 格式
//!
//! Append-only 行式文件，每行一个 tagged JSON（[`Record`]，见 `record.rs`）：
//! `Entry` / `Snapshot` / `Cursor{cursor}` / `DropOldest{count}` / `DropAfter{first_seq,count}`
//! ——跟 [`SessionStore`] 的五个写方法一一对应。`load()`（`load.rs`）从头重放这些行，
//! 喂给和 `Memory` 共用的同一套引擎（[`SessionLog`](agent_store::persist::SessionLog)）
//! 决定最终状态；写路径（`io_thread.rs`）反过来在 IO 线程里养一份同样的引擎，
//! 决定该往文件里追加什么。
//!
//! ## 压实
//!
//! 落一张快照时（`snapshot()`）整份文件被截断只剩这一行——快照之前的 entries 全部
//! 过时，不需要留着（`io_thread.rs::compact` 的实现）。压实之后新的
//! `Entry`/`Cursor`/... 正常追加在这一行后面，`load` 见到 `Snapshot` 记录就重置
//! 累积器，天然只保留「最近一张快照 + 之后的日志」。
//!
//! ## IO 线程与 fire-and-forget
//!
//! 五个写方法只是把消息塞进一个 `mpsc::Sender`（无界，`send` 从不阻塞——actor 不等
//! 磁盘），真正的文件操作全部发生在构造时起的那个专用线程上。任何一次写失败
//! （权限、磁盘满、路径不存在……）都只经构造时传入的 `on_error` 上报一次，绝不
//! panic，内存侧（调用方自己的 `History`）完全不受影响——这就是 fire-and-forget
//! 的字面意思。
//!
//! `load()` 是唯一的例外：它需要马上给出一个值，所以先 [`flush`](Jsonl::flush)
//! 排干队列（确认前面所有写入真的落了盘，不只是入队），再直接在调用线程上读文件、
//! 校验、重放——不经过 IO 线程，因为一份「刚构造、代表进程重启后」的 `Jsonl`
//! 压根没有活的镜像可问，只能从文件本身重建。

mod error;
mod io_thread;
mod load;
mod record;

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde::Serialize;
use serde::de::DeserializeOwned;

use agent_store::history::{Entry, Snapshot};
use agent_store::persist::LoadOutcome;
use agent_store::SessionStore;

pub use error::SessionStoreError;

use io_thread::Msg;

pub struct Jsonl<K, V, M> {
    path: PathBuf,
    tx: Mutex<Option<Sender<Msg<K, V, M>>>>,
    handle: Mutex<Option<JoinHandle<()>>>,
    on_error: Arc<dyn Fn(SessionStoreError) + Send + Sync>,
}

impl<K, V, M> Jsonl<K, V, M>
where
    K: Clone + Serialize + DeserializeOwned + Send + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + 'static,
    M: Clone + Serialize + DeserializeOwned + Send + 'static,
{
    /// 起 IO 线程。**从不失败**——即便 `path` 当场就打不开（只读目录、坏路径），
    /// 构造仍然成功，失败经 `on_error` 报一次（`io_thread` 模块文档），后续写入
    /// 静默吞掉。这是刻意的：fire-and-forget 的端口不该在构造这一步就引入一个
    /// `Result`，那会诱使调用方去处理一个「万一失败怎么办」的分支——恰恰是这个
    /// 端口存在的意义要挡掉的那类分支。
    pub fn new(path: impl Into<PathBuf>, on_error: impl Fn(SessionStoreError) + Send + Sync + 'static) -> Self {
        let path = path.into();
        let on_error: Arc<dyn Fn(SessionStoreError) + Send + Sync> = Arc::new(on_error);
        let (tx, rx) = mpsc::channel();
        let handle = {
            let path = path.clone();
            let on_error = on_error.clone();
            std::thread::spawn(move || io_thread::run(path, rx, on_error))
        };
        Jsonl { path, tx: Mutex::new(Some(tx)), handle: Mutex::new(Some(handle)), on_error }
    }

    /// 排干队列：调用返回时，此前所有写方法产生的写入都已经真正处理完（落盘或者
    /// 确认放弃），不只是「已经入队」。`load()` 内部会先调它——见模块文档。
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
            let _ = tx.send(msg); // 发送失败 = IO 线程已经不在了，静默丢弃
        }
    }
}

impl<K, V, M> SessionStore<K, V, M> for Jsonl<K, V, M>
where
    K: Clone + Serialize + DeserializeOwned + Send + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + 'static,
    M: Clone + Serialize + DeserializeOwned + Send + 'static,
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

    fn load(&self) -> LoadOutcome<K, V, M> {
        self.flush();
        load::load(&self.path, self.on_error.as_ref())
    }
}

/// **排干时机**：先关发送端（IO 线程的 `recv()` 循环见到 channel 关闭才会退出，
/// 不然 `join` 会永远等一条不会来的消息），再 `join`——`join` 之前所有已入队的消息
/// 都会被 IO 线程处理完，这就是「drop 时排干」（issue 011 硬约束）。
impl<K, V, M> Drop for Jsonl<K, V, M> {
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
