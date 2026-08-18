//! `From<RunnerEvent> for SessionEvent`：`agent_runtime::RunnerEvent` → 广播出去的
//! 可序列化事件那条翻译线（拆出 [`super`]，109——`mod.rs` 顶着行数天花板）。
//!
//! 两边变体逐一对应（`SessionEvent` 里那几个「翻译线之外的变体」不在这个
//! `match` 里，见 `super` 模块文档）。**不带 `_` 分支**：`RunnerEvent` 新增一个
//! 变体，这里编译不过，直到给出对应的映射——跟 [`crate::ts_protocol::fixtures`]
//! 的穷举 `cast_sample` 同一条纪律。

use agent_runtime::RunnerEvent;

use super::SessionEvent;

impl From<RunnerEvent> for SessionEvent {
    fn from(ev: RunnerEvent) -> Self {
        match ev {
            RunnerEvent::TextDelta(text) => SessionEvent::TextDelta(text),
            RunnerEvent::ThinkingDelta(text) => SessionEvent::ThinkingDelta(text),
            RunnerEvent::ToolCallStarted { name } => SessionEvent::ToolCallStarted { name },
            RunnerEvent::PreflightDriftAlert(v) => SessionEvent::PreflightDriftAlert(v),
            RunnerEvent::TransportTrouble(text) => SessionEvent::TransportTrouble(text),
            RunnerEvent::ToolExecuting { call_id, request } => {
                SessionEvent::ToolExecuting { call_id, request }
            }
            RunnerEvent::ToolExecuted {
                call_id,
                tool,
                output_len,
                is_error,
            } => SessionEvent::ToolExecuted {
                call_id,
                tool,
                output_len,
                is_error,
            },
            RunnerEvent::TurnGuard {
                usage,
                report,
                adjustments,
            } => SessionEvent::TurnGuard {
                usage,
                report,
                adjustments,
            },
            RunnerEvent::Notice(notice) => SessionEvent::Notice(notice),
            RunnerEvent::OrphanedChild { child, fate } => SessionEvent::OrphanedChild {
                child,
                fate: fate.into(),
            },
            RunnerEvent::UnreadMessages { agent, count } => {
                SessionEvent::UnreadMessages { agent, count }
            }
            RunnerEvent::CompactionApplied {
                turn_id,
                upto,
                summary_id,
            } => SessionEvent::CompactionApplied {
                turn_id,
                upto,
                summary_id,
            },
            RunnerEvent::ToolResultsCleared { turn_id, call_ids } => {
                SessionEvent::ToolResultsCleared { turn_id, call_ids }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{AgentId, Notice};

    use super::super::OrphanFate;
    use super::*;

    /// `RunnerEvent` 的变体逐一对应，穷举 `match` 已经在编译期保证不漏——
    /// 这里额外钉一个运行期样本，防止哪天有人把某个变体的字段悄悄改错映射。
    #[test]
    fn from_runner_event_maps_text_delta() {
        let ev = RunnerEvent::TextDelta(Arc::from("hi"));
        assert_eq!(
            SessionEvent::from(ev),
            SessionEvent::TextDelta(Arc::from("hi"))
        );
    }

    #[test]
    fn from_runner_event_maps_notice() {
        let ev = RunnerEvent::Notice(Notice::TurnStatusChanged {
            status: agent_core::TurnStatus::Idle,
        });
        assert_eq!(
            SessionEvent::from(ev),
            SessionEvent::Notice(Notice::TurnStatusChanged {
                status: agent_core::TurnStatus::Idle
            })
        );
    }

    /// 054：孤儿告警是唯一一条**载荷本身还要再翻一层**的翻译线
    /// （`agent_runtime::OrphanFate` → [`OrphanFate`]），`child` 顺带原样过来。
    #[test]
    fn from_runner_event_maps_orphaned_child_and_its_fate() {
        let ev = RunnerEvent::OrphanedChild {
            child: AgentId::new("root/a1"),
            fate: agent_runtime::OrphanFate::Discarded {
                bytes: 15,
                is_error: false,
            },
        };
        assert_eq!(
            SessionEvent::from(ev),
            SessionEvent::OrphanedChild {
                child: AgentId::new("root/a1"),
                fate: OrphanFate::Discarded {
                    bytes: 15,
                    is_error: false
                },
            }
        );
    }

    /// 109：`CompactionApplied`/`ToolResultsCleared` 是逐字段直译，没有嵌套
    /// 翻译（跟 `OrphanedChild` 的 `fate.into()` 不是同一类）——这里钉住字段
    /// 顺序没有在翻译时被悄悄错配。
    #[test]
    fn from_runner_event_maps_compaction_applied_and_tool_results_cleared() {
        let applied = RunnerEvent::CompactionApplied {
            turn_id: 3,
            upto: 8,
            summary_id: agent_core::SummaryId::new("summary@8"),
        };
        assert_eq!(
            SessionEvent::from(applied),
            SessionEvent::CompactionApplied {
                turn_id: 3,
                upto: 8,
                summary_id: agent_core::SummaryId::new("summary@8"),
            }
        );

        let cleared = RunnerEvent::ToolResultsCleared {
            turn_id: 4,
            call_ids: vec![agent_core::ToolCallId::new("call_1")],
        };
        assert_eq!(
            SessionEvent::from(cleared),
            SessionEvent::ToolResultsCleared {
                turn_id: 4,
                call_ids: vec![agent_core::ToolCallId::new("call_1")],
            }
        );
    }
}
