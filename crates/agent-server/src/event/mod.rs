//! [`SessionEvent`]：actor 广播给外界的一切，经 `tokio::sync::broadcast` 扇出
//! （issue 030）。**这是协议雏形**——032 从这里生成 TS 类型，ARCHITECTURE.md
//! §传输 说的下行 SSE 每一帧就是这个枚举序列化之后的样子（034 起，帧本身是
//! [`Frame`] 信封，见该类型文档；`SessionEvent` 是信封里 `event` 那一半）。
//!
//! # 为什么不是 `agent_runtime::RunnerEvent`
//!
//! `RunnerEvent` 是给同线程内一个 `FnMut` 回调用的——它没有 `Serialize`，也没有
//! 承诺 `'static`（虽然眼下的变体恰好都是 owned，但那是巧合不是契约）。
//! `broadcast` 的载荷必须 `Clone + Send + 'static`，而且要能真的过一遍
//! serde（032 的前提）。`SessionEvent` 是 `RunnerEvent` 的 owned、可序列化翻译，
//! [`From<RunnerEvent>`] 是那条翻译线——两边变体逐一对应，issue 030 的注意事项
//! 原话「`SessionEvent` 的形状别照抄 `RunnerEvent` 的借用结构」在这里落实成
//! 一个独立类型而不是拿 `RunnerEvent` 改个 derive 了事。
//!
//! # `RunnerEvent` 翻译线之外的变体
//!
//! - [`SessionEvent::Undo`] / [`SessionEvent::Redo`]：`/undo` `/redo` `/undo!`
//!   命令的结果（[`UndoOutcome`]，见该类型自己的模块文档——`agent_core::
//!   UndoReport` 的可序列化姊妹类型，034 起还带富化）。`Cancel` 落地成
//!   `Failed(Cancelled)` 之后 actor 的自动擦除策略（027 已裁决，见
//!   `crate::actor::commands` 模块文档）广播的也是这个变体——擦除本质上就是
//!   一次 `undo_turn`，复用同一套事件词汇，不必另开一个「自动擦除」变体。
//! - [`SessionEvent::Lagged`]：订阅者跟丢时的显式补发，见该变体文档。
//! - [`SessionEvent::SessionDied`]：actor panic 之后的终态广播，见该变体文档。
//! - [`SessionEvent::Gap`]：031 的 HTTP 层重连补发逻辑合成的一帧，见该变体文档
//!   ——跟 [`SessionEvent::Lagged`] 哲学同源但触发层不同（那条是 030 的
//!   `tokio::broadcast` 内部跟丢，这条是 031 的 SSE 环形缓冲被挤空）。
//! - [`SessionEvent::AgentTree`]：048。整棵活 agent 树此刻的快照
//!   （`agent_core::Session::agent_tree()` 原样翻译，snapshot 不是 diff——
//!   docs/OBSERVABILITY.md §「snapshot 不是 reconstruct」），由
//!   `agent_runtime::RunnerCtx::with_tree_events` 的独立回调发出（`crate::actor::body`
//!   模块文档），**不经过 `From<RunnerEvent>`**——树快照不是 `RunnerEvent` 的
//!   第十个变体，是独立于它的一条通道（048 issue 范围条款 1）。
//! - [`SessionEvent::TransientSourceFailure`]：runtime 把未改写的 terminal provider
//!   失败事实交给宿主；它不是 `RunnerEvent`，也不应由 runtime 决定展示或脱敏。
//!
//! # 协议决定（032 生成 TS 类型的依据，写进 031 实做记录）
//!
//! `#[serde(rename_all = "snake_case", tag = "type", content = "data")]`——
//! **邻接标签（adjacently tagged）**，不是内部标签（internally tagged）。
//! 原因：本枚举的变体形状五花八门（`TextDelta(Arc<str>)` 这类 newtype 装的是
//! 纯字符串，不是 JSON 对象），内部标签要求每个变体序列化成一个 JSON 对象才能
//! 把 tag 合并进去——`serde` 对「newtype 装非对象」的内部标签枚举会在运行期报错
//! （不是编译期，这个仓库不允许出现这种只在跑起来才发现的坑）。邻接标签对任意
//! 变体形状都成立：`{"type":"text_delta","data":"hi"}`、
//! `{"type":"tool_call_started","data":{"name":"foo"}}`、
//! `{"type":"redo","data":{"type":"nothing"}}`。生成的 TS 判别联合
//! （discriminated union）两种标签风格都能落地，邻接标签更省心。
//! [`UndoOutcome`] 用的是同一套约定。
//!
//! # 五个子模块，各管一件事
//!
//! | 模块 | 职责 |
//! |------|------|
//! | 本文件 | `SessionEvent` 本体 |
//! | [`from_runner`] | `From<RunnerEvent> for SessionEvent`——那条翻译线本身 + 逐变体的映射断言（109 拆出，`mod.rs` 顶着行数天花板） |
//! | [`undo_outcome`] | `UndoOutcome`：undo/redo 结果的可序列化姊妹类型，034 起带 `Blocked` 富化 |
//! | [`orphan_fate`] | 054：`OrphanFate`——轮末孤儿收场的可序列化姊妹类型（`agent_runtime::OrphanFate`） |
//! | [`auto_turn_hold`] | 211：`AutoTurnHold`——一轮自驱动的轮次为什么没自己开，同款姊妹类型 |
//! | [`frame`] | 034：`Frame { agent, event }`——SSE 帧 data 的信封 |

mod auto_turn_hold;
mod frame;
mod from_runner;
mod orphan_fate;
mod transient_source_failure;
mod undo_outcome;

pub use auto_turn_hold::AutoTurnHold;
pub use frame::Frame;
pub use orphan_fate::OrphanFate;
pub use transient_source_failure::{TransientSourceFailureCause, TransientSourceFailureEvent};
pub use undo_outcome::{BlockedCause, UndoOutcome};

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use agent_core::{
    Adjustment, AgentId, AgentTree, DriftVerdict, GuardReport, Notice, SummaryId, TokenUsage,
    ToolCallId, ToolCallRequest,
};

/// 一个 session 广播的一件事。`Clone + Send + 'static`（`broadcast` 的硬要求）
/// 且全部可序列化（032 的前提）——本文件模块文档记了为什么不是直接用
/// `RunnerEvent`，以及 `tag`/`content` 这两个 serde 属性为什么这么选。
///
/// 032：TS 类型从这里生成，`ts` feature 门后面（`crate::ts_protocol`）。**不改
/// 任何 serde 属性**——生成器适配协议，不是协议适配生成器（issue 032 注意事项）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum SessionEvent {
    /// 可见文本的流式增量。
    TextDelta(Arc<str>),
    /// 思维链文本的流式增量。
    ThinkingDelta(Arc<str>),
    /// 流式过程中看到一次工具调用的声明已完整（拿到了名字）。
    ToolCallStarted { name: Arc<str> },
    /// 发前第 1 层告警：`DriftVerdict::Unexpected`。
    PreflightDriftAlert(DriftVerdict),
    /// 一次 `post_stream` 调用没能干净收尾的文本描述。
    TransportTrouble(Arc<str>),
    /// 即将真的执行一个工具。
    ToolExecuting {
        call_id: ToolCallId,
        request: ToolCallRequest,
    },
    /// 工具执行完了。
    ToolExecuted {
        call_id: ToolCallId,
        tool: Arc<str>,
        output_len: usize,
        is_error: bool,
    },
    /// 一轮 `CallProvider` 成功收尾：三层判读 + usage + adjustments。
    TurnGuard {
        usage: TokenUsage,
        report: GuardReport,
        adjustments: Vec<Adjustment>,
    },
    /// loop 自己发的通报（含 `TurnStatusChanged`——轮终态从这里广播出去）。
    Notice(Notice),
    /// `/undo` `/undo!` 的结果，以及取消轮结束后自动擦除的结果
    /// （见本文件模块文档）。
    Undo(UndoOutcome),
    /// `/redo` 的结果。
    Redo(UndoOutcome),
    /// 这个订阅者慢了，`broadcast` 的有界环形缓冲把它还没读到的
    /// `skipped` 条旧事件覆盖掉了——它们永远不会再被这个订阅者看到。
    /// 补发这一条是为了让下游知道自己瞎过一段，而不是无声地以为事件序列
    /// 是连续的（跟 [`tokio::sync::broadcast::error::RecvError::Lagged`]
    /// 的语义一一对应，见 [`crate::handle::Subscription::recv`]）。
    Lagged { skipped: u64 },
    /// actor 线程 panic 了，这是这个 session 广播的最后一条事件——线程即将
    /// 退出，`SessionRegistry` 随后会把它标记为 dead（`reason` 与 registry
    /// 报的死因同源，见 `crate::actor::spawn` 模块文档）。
    SessionDied { reason: String },
    /// **只在 SSE 重连补发时出现，actor 从不广播这个变体**。031 的 HTTP 层
    /// （`crate::http::hub`）给每个广播出去的事件配一个单调帧 id、存进一个有界
    /// 环形缓冲（默认 256 帧）供 `Last-Event-ID` 重连补发；客户端带来的
    /// `Last-Event-ID` 如果比缓冲区当前最旧的一帧还老，中间那些帧已经被挤出去、
    /// 永远补不回来了——补发逻辑就合成这一帧插进流里，`skipped` 是能精确算出的
    /// 缺口大小（`oldest_available_id - last_event_id - 1`，不是估计值）。
    /// 跟 [`SessionEvent::Lagged`] 同一个哲学（瞎过要知道自己瞎过），开一个独立
    /// 变体是因为触发的层和"瞎过"的原因不同：`Lagged` 是 `tokio::broadcast` 内部
    /// 判定的，`Gap` 是重连时按帧 id 算出来的。
    Gap { skipped: u64 },
    /// 048：整棵活 agent 树此刻的快照——`agent_core::Session::agent_tree()`
    /// 原样翻译（推快照不推 diff，本文件模块文档「`RunnerEvent` 翻译线之外的
    /// 变体」）。由 [`agent_runtime::RunnerCtx::with_tree_events`] 的独立回调
    /// 发出（`crate::actor::body`），标 [`agent_core::AgentId::root`]（`crate::
    /// event::frame` 模块文档同一条判据：树是会话级事实，不属于某一个具体
    /// agent 的 `step`）。**不经过 [`From<RunnerEvent>`]**——见该 impl 文档。
    AgentTree(AgentTree),
    /// 054：轮末孤儿告警——模型开了后台子 agent（`spawn(background=true)`），
    /// 父这一轮收尾时却没有 `srv:agent/collect` 去领它。
    /// [`agent_runtime::RunnerEvent::OrphanedChild`] 的可序列化翻译。
    ///
    /// 052 落地时它借的是 [`SessionEvent::TransportTrouble`]，并诚实标注了那个
    /// 名字对不上语义（这不是传输故障，是编排失误）；054 收掉那笔账。
    ///
    /// 帧的 `agent`（[`Frame::agent`]）是**父**——没领是父的编排失误；出事的那个
    /// 子在 `child` 字段里。载荷是事实不是句子（[`OrphanFate`]），措辞归呈现层。
    OrphanedChild { child: AgentId, fate: OrphanFate },
    /// 206：轮末这个 agent 的收件箱里还有 `count` 条 `Deliver::Now` 的话没被读到
    /// ——有人给它发了消息，而它在这一轮里再也没有组装过 provider 请求。
    /// [`agent_runtime::RunnerEvent::UnreadMessages`] 的原样翻译。
    ///
    /// **编排失误的信号，不是错误**：轮次结果照旧。`Deliver::NextTurn` 的条目
    /// 不算在里面（它们本来就该留到下一轮）。载荷是事实不是句子，措辞归呈现层
    /// ——跟 [`SessionEvent::OrphanedChild`] 同一条规矩。
    UnreadMessages { agent: AgentId, count: usize },
    /// 211：**这一轮不是人开的，是留言自己开的**。`remaining` 是扣掉这一轮之后
    /// 还剩几格自驱动预算。[`agent_runtime::RunnerEvent::AutoTurnStarted`] 的原样翻译。
    ///
    /// 这是本仓第一次在没有用户输入的情况下继续消耗 token，所以「这一轮是自己开的」
    /// 和「还能自己开几轮」都不能只进日志（决策 35 §二）。帧的 `agent` 恒是 root。
    AutoTurnStarted { remaining: u32 },
    /// 211：**有留言等着，但这一轮没有自己开**。`pending` 是收件箱里还剩几条，
    /// `reason` 是三种成因之一（[`AutoTurnHold`]）。
    ///
    /// 三种都不是错误，但都必须说出来——**留言原地留着、不丢弃**是三条共有的承诺，
    /// 而一个不说话的「什么都没发生」跟「留言被吞了」在外面长得一模一样。
    AutoTurnHeld { pending: usize, reason: AutoTurnHold },
    /// A terminal provider failure from a request that consumed transient source material.
    /// The payload carries the raw runtime fact; presentation belongs to the embedding host.
    TransientSourceFailure(TransientSourceFailureEvent),
    /// 109：一份摘要被写进状态了——压缩点在时间线上可见的信号。
    /// [`agent_runtime::RunnerEvent::CompactionApplied`] 的原样翻译，见该变体
    /// 文档。`turn_id` 让前端能把这条标记跟对应的 `undo`/`redo` 帧对上号，
    /// 精确地随那一次撤销/重做一起隐藏/恢复（090 的教训：只推事件不够，还要
    /// 能在 undo 之后正确地退回去）。展开这条标记看到的**原文**不从这里取
    /// ——这里只报「发生了」，正文走 `GET /sessions/{id}/compaction_record`
    /// （109 接线约束 1/5）。
    CompactionApplied {
        turn_id: u64,
        upto: usize,
        summary_id: SummaryId,
    },
    /// 109：一批工具调用结果被清除了——同上一条同一条理由，翻译自
    /// [`agent_runtime::RunnerEvent::ToolResultsCleared`]。原文同样不在这里，
    /// 走 `GET /sessions/{id}/compaction_record`。
    ToolResultsCleared {
        turn_id: u64,
        call_ids: Vec<ToolCallId>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 红线 3 精神的直接实检：真的过一遍 serde，不是只看 derive 存在。
    #[test]
    fn session_event_serializes_round_trip() {
        let ev = SessionEvent::Lagged { skipped: 7 };
        let s = serde_json::to_string(&ev).unwrap();
        assert_eq!(serde_json::from_str::<SessionEvent>(&s).unwrap(), ev);

        let died = SessionEvent::SessionDied {
            reason: "boom".to_string(),
        };
        let s = serde_json::to_string(&died).unwrap();
        assert_eq!(serde_json::from_str::<SessionEvent>(&s).unwrap(), died);
    }
}
