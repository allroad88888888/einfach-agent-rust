//! IndexedDB 会话持久化后端（issue 114a）——`SessionStore`（`agent_store`）第二个
//! 做真 IO 的实现，`crate::Jsonl`（`agent-runtime/src/jsonl/`）的兄弟：两者都是
//! 「把 `SessionStore` 的写事件序列化成一份 append-only journal，重放时喂给同一套
//! `agent_store::persist::SessionLog`」这同一个算法的两份落点，只是落点不同
//! （文件 vs. 一个抽象 KV 端口）。
//!
//! ## 为什么落在 `persist/` 而不是像 `jsonl` 一样另起一个顶层模块
//!
//! 114a 的活动范围钉在这个目录之下——同一个分支上并行的另一个 issue（117）在动
//! `runner.rs`/`io_task.rs`/`ctx.rs` 等文件，`persist/` 之下新增文件是两边零文件
//! 重叠、能并行开工的前提。`jsonl` 早于这次拆分就已经是顶层模块，这里不重新挪它。
//!
//! ## 核心设计：把「回放语义」与「IndexedDB 绑定」分开
//!
//! | 文件 | 职责 | 编译目标 |
//! |---|---|---|
//! | [`kv`] | [`kv::KvStore`]：三个操作的异步端口（get/put/按前缀扫），不认识 `SessionStore`/`SessionLog` | 通用 |
//! | [`record`] | journal 里一条记录长什么样、key 怎么编 | 通用 |
//! | [`replay`] | 把 journal 重放进 `SessionLog`——回放语义本身，`KvStore` 的哪个实现供数据它不关心 | 通用 |
//! | [`error`] | [`error::IdbStoreError`]：`on_error` 回调唯一会收到的类型 | 通用 |
//! | [`memory_kv`] | [`memory_kv::MemoryKv`]：`KvStore` 的假实现，纯内存，native 测试用，不需要浏览器 | 通用 |
//! | [`web_kv`] | `KvStore` 的真实现，`web_sys::IdbDatabase` | 仅 `wasm32` |
//! | `blocking` / `worker` / `store` | 把上面几层包成一个真正的 `SessionStore`：工作线程 + channel，`IdbStore`（对外的公开类型） | 仅非 `wasm32` |
//! | `web_store` | 同一件事的浏览器版：`spawn_local` + 一条内存队列，[`web_store::WebIdbStore`]（114c 真正用的那个） | 仅 `wasm32` |
//!
//! `blocking`/`worker`/`store` 那一组是「证明这套引擎写→load→重放的语义是对的」
//! （114a 的验收主证据，用 [`memory_kv::MemoryKv`] 在 native 上跑），不是 wasm
//! 生产环境的最终形态——`wasm32-unknown-unknown` 没有 `std::thread` 这条路可走。
//! wasm 生产环境的接线（114c）就是 `web_store`：直接组装 `kv`/`record`/`replay`
//! 三层，派发换成 `wasm_bindgen_futures::spawn_local`，用不到 [`store::IdbStore`]。
//!
//! ## 与 `Jsonl` 刻意不同的一点：journal 只增不删
//!
//! `Jsonl` 的 `snapshot()` 会截断文件（`set_len(0)`）物理回收空间；`KvStore` 端口
//! 只有 get/put/scan 三个操作，没有 delete——`snapshot()` 落一条新的 journal 记录，
//! 旧记录物理上仍然留在 KV 里。这是已知的、刻意留到之后的**存储空间**问题，不是
//! **回放正确性**问题：重放读到 `Record::Snapshot` 时 `SessionLog::record_snapshot`
//! 照样把之前的 `held` 压实清空，`load()` 的结果不受影响（`replay.rs` 的测试
//! `a_snapshot_record_compacts_exactly_like_jsonl_does` 钉住这一条）。真要物理回收
//! （比如给 `KvStore` 加一个 `delete_range`），是这个设计之后可以加、不改变现有
//! 契约的扩展，不是 114a 的范围。

mod error;
mod kv;
mod memory_kv;
mod record;
mod replay;

#[cfg(not(target_arch = "wasm32"))]
mod blocking;
#[cfg(not(target_arch = "wasm32"))]
mod store;
#[cfg(not(target_arch = "wasm32"))]
mod worker;

#[cfg(target_arch = "wasm32")]
mod web_kv;
#[cfg(target_arch = "wasm32")]
mod web_store;

pub use error::IdbStoreError;
pub use kv::{KvError, KvStore};
pub use memory_kv::MemoryKv;

#[cfg(not(target_arch = "wasm32"))]
pub use store::IdbStore;

#[cfg(target_arch = "wasm32")]
pub use web_kv::IdbDatabaseKv;
#[cfg(target_arch = "wasm32")]
pub use web_store::WebIdbStore;

#[cfg(all(test, not(target_arch = "wasm32")))]
mod parity_tests;
