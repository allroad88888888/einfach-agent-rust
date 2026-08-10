//! 原子图：地址空间 + 唯一的建图入口。
//!
//! | 文件 | 职责 |
//! |------|------|
//! | [`slot`] | 地址空间：`AtomKey` / `Slot` / `ToolCallSlot` / `DerivedKey`，以及「有哪些槽位」（`Slot::ALL`） |
//! | [`slot_default`] | 每个槽位「没有值的时候是什么」——`default_value` 的唯一一处 |
//! | [`visibility`] | 每个槽位跨 agent 时朝哪个方向可读（红线 10 的结构面） |
//! | [`build`] | 构图函数：`source_atom` / `derived_atom` / `build_agent`，建 atom 的唯一入口 |
//!
//! 这一层**不写值**。写值一律走 `command/`（红线 2），这里只负责「哪个槽位、
//! 它叫什么、它是谁建的、谁能读它」。分开的理由是红线 4 与 019 的共同结论：
//! 逻辑键和默认值必须只有一份，而写入点会有很多个。

pub mod build;
pub mod slot;
pub mod slot_default;
pub mod visibility;

pub use build::{AgentStore, DerivedFamily, SourceFamily, build_agent, derived_atom, source_atom};
pub use slot::{AtomKey, DerivedKey, Slot, ToolCallSlot};
pub use visibility::Visibility;
