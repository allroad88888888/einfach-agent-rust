//! [`WebIdbStore`]：`SessionStore` 在**浏览器**里的落点（114c）——[`super::store::
//! IdbStore`] 的 wasm 对应物，`std::thread` + `mpsc` 换成
//! `wasm_bindgen_futures::spawn_local` + 一条内存队列。回放语义、journal 记录格式、
//! key 编码全部复用同一套（[`super::replay`]/[`super::record`]），**一个字节都没有
//! 另起一份**——这正是 114a 把「回放语义」与「IndexedDB 绑定」分开的兑现处。
//!
//! ## 唯一真正的平台差异：`load()` 是同步的，IndexedDB 不是
//!
//! `SessionStore::load` 的签名是同步的（011 的端口设计：actor 是单线程的，写扔给
//! IO 载体，读当场返回）。native 那份靠 `blocking::run_to_completion` 在调用线程上
//! 把一个 future 跑到底；**浏览器里没有这条路**：阻塞当前线程等于把驱动
//! IndexedDB 回调的事件循环一起停住，死锁（同一条实测结论见
//! `agent_transport::fetch_client` 模块文档）。
//!
//! 所以真正的重放发生在 [`WebIdbStore::open`]——一个 `async` 构造器，宿主装配时
//! `await` 它一次；此后 `load()` 读的是这个 store 自己那份**连续维护的 mirror**
//! （`SessionLog`），不再碰 IndexedDB。这不是"缓存了一份可能过期的数据"：
//! `worker.rs` 的整套记账本来就建立在「mirror 与 journal 重放结果恒等」这条不变量
//! 上（落盘写的正是 mirror 已经算好的净效果），这里只是把同一条不变量用在读的
//! 一侧。**刷新页面后的第一次 `load()` 读的是真正的 IndexedDB 重放**，因为那时
//! `open()` 刚跑完——114 验收第三条要验的就是这一条路径。
//!
//! ## 写入：一条队列 + 一个 drain 任务，不是每次 put 各起一个任务
//!
//! journal key 是递增计数器，谁先写谁拿小号；每个写各 `spawn_local` 一次会让
//! IndexedDB 事务的完成顺序决定编号顺序，重放序就跟发生序对不上了。所以入队是
//! 同步的（`SessionStore` 的五个写方法照旧 fire-and-forget、当场返回），出队由
//! **同一时刻至多一个** drain 任务串行做——`draining` 那个闩就是这件事。
//!
//! 写失败的处理与 [`super::worker`] 逐字一致：**每次失败都报、且不推进
//! `next_index`**，下一条记录会重新用同一个 index，不在 journal 里留空洞。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use serde::Serialize;
use serde::de::DeserializeOwned;

use agent_store::SessionStore;
use agent_store::history::{Entry, Snapshot};
use agent_store::persist::{LoadOutcome, SessionLog};

use super::error::IdbStoreError;
use super::kv::KvStore;
use super::record::{Record, journal_key};
use super::replay;

pub struct WebIdbStore<K, V, M, KV> {
    inner: Rc<Inner<K, V, M, KV>>,
    /// 开库那一次重放被拒（journal 里有读不回来的记录）的理由。`Some` 时
    /// `load()` 恒返回 `Refused`——宿主必须硬失败让人先备份现场，不能静默当成
    /// 新会话（`LoadOutcome` 文档里那条「三态化」的理由）。
    refusal: Option<String>,
}

struct Inner<K, V, M, KV> {
    kv: Rc<KV>,
    /// 连续维护的记账镜像，见模块文档——`load()` 的数据源。
    mirror: RefCell<SessionLog<K, V, M>>,
    /// 下一条 journal 记录的编号。只有真正 `put` 成功才推进。
    next_index: Cell<u64>,
    /// 已序列化、还没落进 IndexedDB 的记录，**按发生顺序**。
    queue: RefCell<VecDeque<Vec<u8>>>,
    /// 「此刻有没有一个 drain 任务在跑」。见模块文档「写入」一节。
    draining: Cell<bool>,
    on_error: Rc<dyn Fn(IdbStoreError)>,
}

impl<K, V, M, KV> WebIdbStore<K, V, M, KV>
where
    K: Clone + Serialize + DeserializeOwned + 'static,
    V: Clone + Serialize + DeserializeOwned + 'static,
    M: Clone + Serialize + DeserializeOwned + 'static,
    KV: KvStore + 'static,
{
    /// 开一个 store：**在这里、也只在这里**真的重放一遍 IndexedDB 里的 journal。
    ///
    /// 跟 `IdbStore::spawn` 一样从不失败——重放被拒不是构造失败，是 `load()` 要
    /// 报的三态之一（见 `refusal` 字段）。构造这一步引入 `Result` 会诱使调用方
    /// 在错误的位置处理一件它处理不了的事。
    pub async fn open(kv: KV, on_error: impl Fn(IdbStoreError) + 'static) -> Self {
        let kv = Rc::new(kv);
        let on_error: Rc<dyn Fn(IdbStoreError)> = Rc::new(on_error);
        let (mirror, next_index, refusal) = match replay::replay_all(kv.as_ref()).await {
            Ok((log, count)) => (log, count, None),
            Err(error) => {
                on_error(error.clone());
                // `next_index` 从 0 起步：跟 `replay::seed` 的退化行为一致。这一态
                // 下 `load()` 恒 `Refused`，宿主本来就该硬失败、不接着写。
                (SessionLog::new(), 0, Some(error.to_string()))
            }
        };
        WebIdbStore {
            inner: Rc::new(Inner {
                kv,
                mirror: RefCell::new(mirror),
                next_index: Cell::new(next_index),
                queue: RefCell::new(VecDeque::new()),
                draining: Cell::new(false),
                on_error,
            }),
            refusal,
        }
    }

    /// 序列化一条记录、排进队列、确保有一个 drain 任务在跑。**同步返回**。
    ///
    /// 序列化失败不该发生（红线 3：primitive 必须全部可序列化），这里只做防御，
    /// 不当成「KV 坏了」处理——跟 [`super::worker::write`] 同一条取舍。
    fn enqueue(&self, record: Record<K, V, M>) {
        let Ok(bytes) = serde_json::to_vec(&record) else {
            return;
        };
        self.inner.queue.borrow_mut().push_back(bytes);
        drain(Rc::clone(&self.inner));
    }
}

/// 保证「此刻有一个 drain 任务在跑」。已经有了就直接返回——新排进去的记录会被
/// 那个还在跑的任务看到（它每一圈都重新看队列）。
fn drain<K, V, M, KV>(inner: Rc<Inner<K, V, M, KV>>)
where
    K: Serialize + 'static,
    V: Serialize + 'static,
    M: Serialize + 'static,
    KV: KvStore + 'static,
{
    if inner.draining.get() {
        return;
    }
    inner.draining.set(true);
    wasm_bindgen_futures::spawn_local(async move {
        loop {
            // 借用必须在 `.await` 之前结束：drain 任务与 `enqueue` 跑在同一条
            // 线程上，跨 await 持有 `RefMut` 就是一次必然的 `already borrowed`。
            let next = inner.queue.borrow_mut().pop_front();
            let Some(bytes) = next else { break };
            let key = journal_key(inner.next_index.get());
            match inner.kv.put(&key, &bytes).await {
                Ok(()) => inner.next_index.set(inner.next_index.get() + 1),
                Err(error) => (inner.on_error)(IdbStoreError::Kv(error)),
            }
        }
        inner.draining.set(false);
    });
}

impl<K, V, M, KV> SessionStore<K, V, M> for WebIdbStore<K, V, M, KV>
where
    K: Clone + Serialize + DeserializeOwned + 'static,
    V: Clone + Serialize + DeserializeOwned + 'static,
    M: Clone + Serialize + DeserializeOwned + 'static,
    KV: KvStore + 'static,
{
    fn append(&self, entry: &Entry<K, V, M>) {
        self.inner.mirror.borrow_mut().record_append(entry);
        self.enqueue(Record::Entry(entry.clone()));
    }

    fn drop_oldest(&self, count: usize) {
        // 落盘的是 mirror 已经吸收过 `boundary` 之后的净效果，不是调用方给的原始
        // `count`——理由见 `crate::jsonl::io_thread` 模块文档「压实之后为什么不能
        // 落原始值」，`worker.rs` 那一份逐字适用。
        let removed = self.inner.mirror.borrow_mut().record_drop_oldest(count);
        self.enqueue(Record::DropOldest { count: removed });
    }

    fn drop_after(&self, first_seq: u64, count: usize) {
        self.inner
            .mirror
            .borrow_mut()
            .record_drop_after(first_seq, count);
        self.enqueue(Record::DropAfter { first_seq, count });
    }

    fn set_cursor(&self, cursor: usize) {
        // 同上：落换算之后的相对游标。
        let cursor = {
            let mut mirror = self.inner.mirror.borrow_mut();
            mirror.record_cursor(cursor);
            mirror.relative_cursor()
        };
        self.enqueue(Record::Cursor { cursor });
    }

    fn snapshot(&self, snap: &Snapshot<K, V>) {
        self.inner.mirror.borrow_mut().record_snapshot(snap);
        self.enqueue(Record::Snapshot(snap.clone()));
    }

    /// 见模块文档「唯一真正的平台差异」：读的是 mirror，不是 IndexedDB。真正的
    /// 重放在 [`WebIdbStore::open`] 里已经做过了。
    fn load(&self) -> LoadOutcome<K, V, M> {
        if let Some(reason) = &self.refusal {
            return LoadOutcome::Refused {
                reason: reason.clone(),
            };
        }
        self.inner
            .mirror
            .borrow()
            .to_loaded()
            .map_or(LoadOutcome::Absent, LoadOutcome::Loaded)
    }
}
