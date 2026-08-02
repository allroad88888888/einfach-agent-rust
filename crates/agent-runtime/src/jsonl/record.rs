//! [`Record`]：Jsonl 文件里一行长什么样。
//!
//! 每行一个 tagged JSON 对象，`serde(tag = "kind")` 内部标签——`{"kind":"entry",...}`。
//! 五个变体对应 [`SessionStore`](agent_store::SessionStore) 的五个写方法，一一对应，
//! 没有多余的第六种：`load` replay 的时候按 `kind` 分发进
//! [`SessionLog`](agent_store::persist::SessionLog) 对应的 `record_*` 方法
//! （见 `jsonl/load.rs`），写路径（`jsonl/io_thread.rs`）反过来把每个 `SessionStore`
//! 调用序列化成对应的一行——两条路径的变体必须一一对应，否则会出现「写得出来、
//! 读不回去」的记录。

use serde::{Deserialize, Serialize};

use agent_store::history::{Entry, Snapshot};

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum Record<K, V, M> {
    Entry(Entry<K, V, M>),
    Snapshot(Snapshot<K, V>),
    Cursor { cursor: usize },
    DropOldest { count: usize },
    DropAfter { first_seq: u64, count: usize },
}
