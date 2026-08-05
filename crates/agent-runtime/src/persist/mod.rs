//! 会话持久化的接线层（issue 027）：把 011 的 `SessionStore` 端口接到 026 的
//! `Session` 上。`agent-core`/`agent-store` 都不认识对方（红线 7 + 泛型隔离），
//! 这一层就是那座桥——只有它同时认识 `Session`（agent-core）和
//! `SessionStore<K,V,M>`（agent-store），把两者缝起来是运行时层该干的事。
//!
//! | 文件 | 职责 |
//! |------|------|
//! | [`meta`] | `PersistedMeta`：`EntryMeta` 的可落盘姊妹类型 + 两个方向的转换 |
//! | [`sync`] | 每条命令之后把 `Session` 的变化转发进 `SessionStore`（append/cursor/drop）；也带 `seed_after_recover`——恢复之后必调一次，见其文档「真 bug」一节 |
//! | [`snapshot`] | 快照节奏：每 N 个 turn 落一张 |
//! | [`recover`] | 崩溃恢复：`SessionStore::load()` → 翻译 → `Session::restore` |
//! | [`backend`] | 挑后端：有路径 `Jsonl`，没有 `Memory` |

pub mod backend;
pub mod meta;
pub mod recover;
pub mod snapshot;
pub mod sync;

pub use backend::open_backend;
pub use meta::{PersistedMeta, UnknownLabel};
pub use recover::{RecoverError, has_unresolved_tool_calls, recover};
pub use snapshot::maybe_snapshot;
pub use sync::{seed_after_recover, sync};

/// 固定了 `K`/`V`/`M` 三个类型参数之后的 `SessionStore` trait object——
/// [`crate::ctx::RunnerCtx`] 挂的就是它。`K=AtomKey`/`V=AgentValue` 是
/// `Session` 的落盘键值类型（026），`M=PersistedMeta` 是这一层特有的、可
/// `Deserialize` 的元数据（`agent_core::EntryMeta` 本身不行，见 [`meta`]）。
pub type SessionBackend =
    dyn agent_store::SessionStore<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>;
