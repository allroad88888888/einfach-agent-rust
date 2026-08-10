//! M12 压缩主干的**纯策略层**：什么时候按哪一档、按下去清谁/摘哪段。
//!
//! 只装判断，不装状态、不装编排。写状态是 [`crate::command`] 的事（101 的
//! `Session::clear_tool_results`、107 的 `Session::apply_summary`），把决定真的
//! 执行掉是 `agent-runtime` 的事（108 的接线）。这里每一个函数都必须是纯函数
//! （红线 1）、零模型相关判断（红线 12）——同 `crate::cache` 那层判读的纪律。
//!
//! | 文件 | 一件事 |
//! |---|---|
//! | [`pressure`] | 窗口压力够不够开火（两档共用的唯一数值判据） |
//! | [`protected_region`] | 「最近 N 轮」这条保护区的线画在哪（两档共用的同一条线） |
//! | [`clear_policy`] | 第 2 档：这一轮该清哪些工具结果（102） |
//! | [`ladder`] | 阶梯：这一轮该走哪一档（108） |

pub mod clear_policy;
pub mod ladder;
mod pressure;
mod protected_region;

pub use clear_policy::{
    ClearParams, DEFAULT_PROTECT_RECENT_TURNS, DEFAULT_TRIGGER_PERCENT, tool_results_to_clear,
};
pub use ladder::{LadderAction, next_action};
