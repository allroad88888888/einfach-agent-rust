//! [`cast_sample`]：骨架 → 真正想要的样本值（拆出 [`super`]，109——那个文件顶着
//! 行数天花板）。**穷举，没有 `_` 分支**——新增 `SessionEvent` 变体时这里编译
//! 不过，直到给出对应的样本（issue 032「编译器保证全覆盖」）。

use std::sync::Arc;

use agent_core::{
    Adjustment, AgentActivity, AgentId, AgentNode, AgentTree, DriftVerdict, GuardReport, Location,
    Notice, ReconcileVerdict, Reversibility, Segment, SummaryId, TokenUsage, ToolCallId,
    ToolCallRequest, TurnStatus, WindowVerdict,
};

use crate::{
    OrphanFate, SessionEvent, TransientSourceFailureCause, TransientSourceFailureEvent,
    UndoOutcome,
};

pub(super) fn cast_sample(ev: SessionEvent) -> SessionEvent {
    match ev {
        SessionEvent::TextDelta(_) => SessionEvent::TextDelta(Arc::from("streamed answer chunk")),
        SessionEvent::ThinkingDelta(_) => {
            SessionEvent::ThinkingDelta(Arc::from("considering which tool to call"))
        }
        SessionEvent::ToolCallStarted { .. } => SessionEvent::ToolCallStarted {
            name: Arc::from("srv:fs/read"),
        },
        SessionEvent::PreflightDriftAlert(_) => {
            SessionEvent::PreflightDriftAlert(DriftVerdict::Unexpected {
                segment: Segment::Tools,
            })
        }
        SessionEvent::TransportTrouble(_) => {
            SessionEvent::TransportTrouble(Arc::from("post_stream ended without a stop reason"))
        }
        SessionEvent::ToolExecuting { .. } => SessionEvent::ToolExecuting {
            call_id: ToolCallId::new("call_1"),
            request: ToolCallRequest {
                tool: Arc::from("srv:fs/read"),
                input: Arc::new(serde_json::json!({ "path": "/tmp/a.txt" })),
                location: Location::Server,
                reversibility: Reversibility::Pure,
            },
        },
        SessionEvent::ToolExecuted { .. } => SessionEvent::ToolExecuted {
            call_id: ToolCallId::new("call_1"),
            tool: Arc::from("srv:fs/read"),
            output_len: 128,
            is_error: false,
        },
        SessionEvent::TurnGuard { .. } => SessionEvent::TurnGuard {
            usage: TokenUsage {
                prompt: 1000,
                completion: 64,
                cached: Some(900),
            },
            report: GuardReport {
                drift: DriftVerdict::Clean,
                reconcile: ReconcileVerdict::Match {
                    predicted: 900,
                    actual: 900,
                },
                window: WindowVerdict::Healthy {
                    turns: 4,
                    hit_percent: 92,
                    low_streak: 0,
                },
            },
            adjustments: vec![Adjustment::TemperatureOverridden {
                wanted: 0.7,
                used: 1.0,
            }],
        },
        SessionEvent::Notice(_) => SessionEvent::Notice(Notice::TurnStatusChanged {
            status: TurnStatus::Idle,
        }),
        // 034：样本挑 `Blocked`（不是 `Applied`）——这是唯一带富化字段
        // （label/tool/call_id）的分支，选它才能让 TS 的 `satisfies` 检查真的
        // 照到这三个新字段的形状，而不是让协议改动躲过 fixtures 这道实检。
        SessionEvent::Undo(_) => SessionEvent::Undo(UndoOutcome::Blocked {
            entries: 1,
            barrier_seq: 5,
            label: "tool_result".to_string(),
            tool: Some("srv:shell/exec".to_string()),
            call_id: Some("call_1".to_string()),
        }),
        SessionEvent::Redo(_) => SessionEvent::Redo(UndoOutcome::Nothing),
        SessionEvent::Lagged { .. } => SessionEvent::Lagged { skipped: 7 },
        SessionEvent::SessionDied { .. } => SessionEvent::SessionDied {
            reason: "actor panicked: boom".to_string(),
        },
        SessionEvent::Gap { .. } => SessionEvent::Gap { skipped: 3 },
        // 048：样本挑「root + 一个子 agent」而不是只有 root——`AgentNode` 的
        // `parent`/`depth` 两个字段在单节点样本上永远是 `None`/`0`，选一个
        // 带子 agent 的样本才能让 TS 的 `satisfies` 检查真的照到「非 root
        // 节点长什么样」这个形状，跟上面 `Undo` 选 `Blocked` 同一条理由。
        SessionEvent::AgentTree(_) => SessionEvent::AgentTree(AgentTree {
            nodes: vec![
                AgentNode {
                    id: AgentId::root(),
                    parent: None,
                    depth: 0,
                    task: Some("帮我查一下今天的天气".to_string()),
                    activity: AgentActivity::Working {
                        tools: vec!["srv:agent/spawn".to_string()],
                    },
                },
                AgentNode {
                    id: AgentId::root().child(1),
                    parent: Some(AgentId::root()),
                    depth: 1,
                    task: Some("查天气".to_string()),
                    activity: AgentActivity::Done { truncated: false },
                },
            ],
        }),
        // 054：样本挑 `Discarded`（不是 `Despawned`）——它是三个变体里字段最多
        // 的那个（`bytes` + `is_error`），选它才能让 TS 的 `satisfies` 检查真的
        // 照到嵌套那一层的字段形状，跟上面 `Undo` 选 `Blocked`、`AgentTree` 选
        // 带子 agent 的样本同一条理由。`child` 挑一个非 root 的 id：孤儿按定义
        // 就不可能是 root（`despawn_child` 拒绝拆 root）。
        SessionEvent::OrphanedChild { .. } => SessionEvent::OrphanedChild {
            child: AgentId::root().child(1),
            fate: OrphanFate::Discarded {
                bytes: 128,
                is_error: false,
            },
        },
        SessionEvent::TransientSourceFailure(_) => {
            SessionEvent::TransientSourceFailure(TransientSourceFailureEvent {
                epoch: 7,
                cause: TransientSourceFailureCause::TransportHttp {
                    status: 502,
                    body: "upstream diagnostic".to_string(),
                },
            })
        }
        // 109：压缩点在时间线上可见的两条信号。`upto`/`turn_id` 挑非零值，
        // `call_ids` 挑非空——同上面几处「挑带字段的分支」同一条理由,让 TS 的
        // `satisfies` 检查真的照到这些字段的形状。
        SessionEvent::CompactionApplied { .. } => SessionEvent::CompactionApplied {
            turn_id: 3,
            upto: 12,
            summary_id: SummaryId::new("summary@12"),
        },
        SessionEvent::ToolResultsCleared { .. } => SessionEvent::ToolResultsCleared {
            turn_id: 3,
            call_ids: vec![ToolCallId::new("call_1"), ToolCallId::new("call_2")],
        },
    }
}
