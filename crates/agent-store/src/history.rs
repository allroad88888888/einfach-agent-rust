//! 事务日志式的 command log —— undo/redo、持久化、崩溃恢复、审计回放共用的**同一份**
//! 记录（`docs/STATE-MODEL.md` §「Command log」）。
//!
//! 009 做「记录」这一半：日志结构 + 唯一的记录入口。017 加游标与两层粒度的 undo/redo。
//! 018 加日志上限与裁剪事件。还没做的：019 已 evict atom 的按需重建。
//!
//! **History 不碰 store**：undo/redo 只挪游标、把该应用的条目克隆出来
//! （[`UndoOutcome`]），把 `prev`/`next` 写回状态是上层 applier 的事。这条分界让日志侧
//! 保持可测、可序列化、与状态规模无关，也是 019 能单独长在上面的原因。
//!
//! ## 为什么是事务日志而不是快照式
//!
//! 每条 entry 自带完整逆操作（`prev` 是写入前当场捕获的），于是它**可截断**（丢最老的
//! 不影响剩余条目回滚）、**可序列化**（键是泛型逻辑键 `K`，不是对象引用）、**代价与
//! 状态规模脱钩**（一次 undo 是 O(本条 changes 数)）。快照式必须回溯扫描前序历史才能
//! 找到某个键的上一个值，截断即永久丢失可回滚性。
//!
//! ## 文件分工
//!
//! | 文件 | 职责 |
//! |------|------|
//! | `history/log.rs` | 日志结构本身：`Change` / `Entry` / `History`，`append` 铸 seq。对 store 一无所知 |
//! | `history/record.rs` | 记录入口：把一次 store 写入变成一条 `Change`（`record_set`） |
//! | `history/cursor.rs` | 游标：两层粒度的 undo/redo，产出 `UndoOutcome`。同样对 store 一无所知 |
//! | `history/cap.rs` | 日志上限与裁剪事件：`set_cap` / `take_drop_events` / `DropEvent`（018） |
//! | `history/apply.rs` | applier：把 undo/redo 产物写回 store，缺席的 atom 由调用方的 get-or-create `resolve` 重建（019） |
//! | `history/snapshot.rs` | 快照长什么样：全部 primitive 的「逻辑键 → 值」清单，可落盘。对 store 一无所知（010） |
//! | `history/capture.rs` | 采集与灌回：`Store` 与 `Snapshot` 之间的整份搬运（`capture` / `restore`，010） |
//! | `history/parts.rs` | `History` 的持久化边界：`to_parts` / `from_parts`，重建时校验不变量（010） |
//! | `history/apply_roundtrip.rs` | 仅测试：把上面几个接起来跑一遍全链路（图 → 记录 → undo → applier → redo） |
//! | `history/snapshot_roundtrip.rs` | 仅测试：快照全链路（采集 → 存盘 → 全新构图 → restore → `apply_next` 推到最新） |
//!
//! 这一刀不只是行数：红线 4「落盘的键不能是 `AtomId`」在本 crate 的形状是**日志结构
//! 这个文件里根本没有 `AtomId` 这个符号** —— 键的语义由上层选择，进程内句柄只出现在
//! 记录入口一侧，写进日志的是 `K`。`scripts/check-invariants.sh` 的红线 4 检查
//! （同一文件里既有 `Serialize` 派生又出现 `AtomId`）因此在结构上永不可能被触发，
//! 而不是靠人记得别写。

mod apply;
mod cap;
mod capture;
mod cursor;
mod log;
mod parts;
mod record;

/// 010 的公开面 issue 原文点名在 `snapshot.rs`。实现被红线 4 的检查器劈成了两个文件
/// （可落盘的结构一侧不许出现 `AtomId`），所以这个模块是 `pub` 的、并把 `capture` /
/// `restore` 再导出一次 —— `history::snapshot::{Snapshot, capture, restore}` 因此仍然
/// 是有效路径，调用方不必知道这一刀切在哪。
pub mod snapshot;

/// 全链路验收（017 验收第一条）住在自己的文件里：它是唯一需要同时看见 `Store` /
/// `AtomId` 和日志的地方，放进 `cursor.rs` 会把「游标怎么动」和「产物怎么落回状态」
/// 两件事糊在一个文件里。
#[cfg(test)]
mod apply_roundtrip;

/// 快照的全链路验收（010 三条验收），同上的理由自成一个文件。
#[cfg(test)]
mod snapshot_roundtrip;

pub use apply::{apply_next, apply_prev};
pub use cap::DropEvent;
pub use cursor::UndoOutcome;
pub use log::{Change, Entry, History};
pub use parts::InvalidHistory;
pub use record::record_set;
pub use snapshot::{Snapshot, capture, restore};
