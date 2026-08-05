//! Agent atom store — fork of `@einfach/core` Rust implementation,
//! generified for arbitrary value types and deXMLized.
//!
//! Core components:
//! - `AtomValue`: trait for value types that can be stored
//! - `Store<V>`: atom state container with dependency tracking
//! - `AtomFamily<K>`: keyed cache of atoms
//! - `ReadArgs<V>`, `WriteArgs<V>`: access patterns for read/write functions
//! - `CellListener`, `SubscriptionId`: subscription plumbing
//! - `history`: 事务日志式 command log（`Change` / `Entry` / `History` / `record_set`）
//!   与它的游标（两层粒度的 undo/redo，`UndoOutcome`），加日志上限与裁剪事件
//!   （`DropEvent`）、把产物写回 store 的 applier（`apply_prev` / `apply_next`）、
//!   快照与恢复（`Snapshot` / `capture` / `restore`）与日志的持久化边界
//!   （`History::to_parts` / `from_parts` / `InvalidHistory`）
//! - `persist`: 会话持久化端口（`SessionStore`，issue 011）与它的内存实现
//!   （`Memory`）。真正落盘的 `Jsonl` 做 IO，红线 7 不许它进这个 crate，住在
//!   `agent-runtime`。

pub mod family;
pub mod history;
pub mod ids;
pub mod persist;
pub mod store;

// Re-exports for convenience
pub use family::AtomFamily;
pub use history::{
    Change, DropEvent, Entry, History, InvalidHistory, Snapshot, UndoOutcome, apply_next,
    apply_prev, capture, record_set, restore,
};
pub use ids::AtomId;
pub use persist::{LoadOutcome, LoadedSession, Memory, SessionStore};
pub use store::{AtomValue, CellListener, ReadArgs, Store, SubscriptionId, WriteArgs};
