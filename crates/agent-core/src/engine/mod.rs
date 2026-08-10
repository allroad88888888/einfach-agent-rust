//! loop 的接缝词汇：**core 决定该发生什么，宿主决定怎么发生。**
//!
//! 五个文件各管一件事：[`event`] 进来的、[`effect`] 出去的、[`state`] 里剩下的
//! `Session` 词汇（`TurnStatus`/`Failure`/`ToolSlot`/`SlotState`）、[`notice`] 是
//! `Effect::Emit` 的载荷、[`epoch`] 是红线 6 的世代标记。
//!
//! **001 定了这些词汇的形状与 epoch 闸的位置**；002/016/003 曾经在这里填过一张
//! `TurnStatus` 5 态 × `Event` 7 变体的转移表（`engine::step` 驱动
//! `TurnState`）。**026 把转移语义原生迁进了原子图（[`crate::command::Session`]），
//! 027 把 runner/CLI 换接到 `Session` 之后，`engine::step` 那一路（连同
//! `engine::transitions` 与 `TurnState` 本身）退役**——完整的转移表现在唯一住在
//! `crate::command::transitions`，epoch 闸唯一住在 `Session::step`，语义一字未改，
//! `docs/issues/026-state-into-atoms.md` 的等价重写对照表就是「这一条行为原来
//! 在哪、现在在哪」的账本。
//!
//! 红线 7：这里没有 IO，**也没有 `Instant::now()`** —— 超时是宿主注入的
//! [`Event::Timeout`]，计时器活在 runner（012）里。红线 12：这里一条模型相关的
//! 判断都没有，结构上也做不到——依赖方向是 providers → core，能力位那张表连
//! 类型都在 core 之外，想读也读不到。

pub mod effect;
pub mod epoch;
pub mod event;
pub mod notice;
pub mod state;

#[cfg(test)]
mod event_tests;

pub use effect::Effect;
pub use epoch::Epoch;
pub use event::Event;
pub use notice::Notice;
pub use state::{Failure, SlotState, ToolSlot, TurnStatus};
