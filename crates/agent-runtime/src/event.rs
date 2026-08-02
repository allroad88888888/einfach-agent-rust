//! [`RunnerEvent`]：runner 说给宿主（CLI/server）听的一切，经 [`crate::RunnerCtx`]
//! 的回调送出去。
//!
//! 判据跟 `agent_core::Notice` 的判据是同一条——**loop 自己看不见的才在这里**。
//! 反过来，loop 已经用 `Effect::Emit(Notice)` 说过的事（`TurnStatusChanged` /
//! `ToolOutputTruncated` / `ProtocolViolation` / `Retrying`）不重复定义一遍，
//! 直接透传 [`Notice`] 本身（见 [`RunnerEvent::Notice`]）——这里新增的变体都是
//! **只有 runner 自己知道**的事：流式增量（累积器活在 runner，001 判断 1）、
//! 发前 drift 告警（024：判读的输入宿主全部持有，走不了 `Notice`）、工具真的
//! 执行了什么（`Effect::ExecuteTool` 只带名字和输入，「真的跑了、结果多长」
//! 只有 runner 知道）、一轮结束的完整 `GuardReport`。

use std::sync::Arc;

use agent_core::cache::{DriftVerdict, GuardReport};
use agent_core::{Adjustment, AgentId, Notice, TokenUsage, ToolCallId, ToolCallRequest};

/// 一件 runner 想让宿主看见的事，**加上「谁说的」**（029）。
///
/// # 为什么是外面包一层，不是往 [`RunnerEvent`] 每个变体里塞 `agent`
///
/// 归属是**这条事件的元数据**，不是任何一个变体的载荷：`TextDelta` 之所以要带
/// agent，跟 `ToolExecuted` 之所以要带 agent 是同一个理由（多 agent 并行输出时
/// 分得出谁说的），把同一句话在九个变体里各写一遍，加第十个变体时就有第十次
/// 漏写的机会。包一层之后「每条事件都有归属」是类型事实，不是纪律。
///
/// 顺带的结果是 `RunnerEvent` 的形状一个字节没动——029 的注意事项写着
/// 「core 的 `Notice` 不动，那是 031/032 的协议面自己组织的事」，同一条理由对
/// `agent-server` 的 `SessionEvent`（已经跨 SSE 的公开枚举）同样成立：它翻译
/// `RunnerEvent` 的那条 `From` 不该因为 runner 内部长出多 agent 而被迫改形状，
/// 归属要不要进协议、进成什么样，是 031/032 自己的判断。
#[derive(Clone, Debug)]
pub struct AgentEvent {
    /// 这件事出自哪个 agent 的 `step` / 流。
    pub agent: AgentId,
    pub event: RunnerEvent,
}

/// runner 想让宿主看见的一件事。**不是命令**：宿主可以只打印、可以推 SSE、
/// 可以丢掉——跟 `Notice` 的文档注释是同一条精神。
#[derive(Clone, Debug)]
pub enum RunnerEvent {
    /// 可见文本的流式增量。累积在 runner 这边完成（accumulator 活在宿主，
    /// ADAPTER.md §时序），不进 loop——001 判断 1 的直接后果。
    TextDelta(Arc<str>),
    /// 思维链文本的流式增量。
    ThinkingDelta(Arc<str>),
    /// 流式过程中看到一次工具调用的声明已完整（拿到了名字）——参数可能还在流。
    /// 这是「实时」的第一手信号，跟下面 [`RunnerEvent::ToolExecuting`]（拿到完整
    /// 参数、真的要执行了）是两个不同时刻。
    ToolCallStarted { name: Arc<str> },

    /// 发前第 1 层告警：`DriftVerdict::Unexpected`。**只在这一种判读结果时才发**
    /// ——`Clean`/`Expected` 不算事故，会在这一轮成功收尾时随
    /// [`RunnerEvent::TurnGuard`] 一起打出来；但事故必须在花钱之前立刻可见，
    /// 等不到那一步（这一轮可能失败、超时、被取消，`TurnGuard` 根本不会发生）。
    PreflightDriftAlert(DriftVerdict),

    /// 一次 `post_stream` 调用没能干净收尾（连接中途断开、连接失败、非 200
    /// 响应）——文本描述，进日志/CLI，不参与任何判断（那是 `class` 的事，
    /// 已经在随后的 loop 事件里体现）。
    TransportTrouble(Arc<str>),

    /// 即将真的执行一个工具。`request` 是宿主按 002 合并记录的裁决现造的
    /// 「发起时快照」——`tool`/`input` 是模型给的，`location`/`reversibility`
    /// 由 [`crate::tool_table::ToolTable::snapshot`] 按名字查表补全。
    ToolExecuting { call_id: ToolCallId, request: ToolCallRequest },
    /// 工具执行完了。`output_len` 是原始（未截断）字节数——截断发生在 core
    /// 边界（决策 19），这里报的是 executor 真正吐出来的长度。
    ToolExecuted { call_id: ToolCallId, tool: Arc<str>, output_len: usize, is_error: bool },

    /// 一轮 `CallProvider` 成功收尾：三层判读 + usage + adjustments 一起给宿主。
    /// **每次成功的 provider 调用都发一次**，不是整个 `TurnState` 只发一次——
    /// 重试之后的那次成功调用一样要看得见它自己的 usage 和判读。
    TurnGuard { usage: TokenUsage, report: GuardReport, adjustments: Vec<Adjustment> },

    /// loop 自己发的通报，原样透传——见本文件顶部的判据。
    Notice(Notice),
}
