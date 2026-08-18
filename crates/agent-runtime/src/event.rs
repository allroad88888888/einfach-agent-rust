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
use agent_core::{
    Adjustment, AgentId, Notice, SummaryId, TokenUsage, ToolCallId, ToolCallRequest,
};

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
    ToolExecuting {
        call_id: ToolCallId,
        request: ToolCallRequest,
    },
    /// 工具执行完了。`output_len` 是原始（未截断）字节数——截断发生在 core
    /// 边界（决策 19），这里报的是 executor 真正吐出来的长度。
    ToolExecuted {
        call_id: ToolCallId,
        tool: Arc<str>,
        output_len: usize,
        is_error: bool,
    },

    /// 一轮 `CallProvider` 成功收尾：三层判读 + usage + adjustments 一起给宿主。
    /// **每次成功的 provider 调用都发一次**，不是整个 `TurnState` 只发一次——
    /// 重试之后的那次成功调用一样要看得见它自己的 usage 和判读。
    TurnGuard {
        usage: TokenUsage,
        report: GuardReport,
        adjustments: Vec<Adjustment>,
    },

    /// loop 自己发的通报，原样透传——见本文件顶部的判据。
    Notice(Notice),

    /// 054：轮末清算的孤儿告警——模型开了后台子 agent（`spawn(background=true)`），
    /// 父这一轮收尾时却没有 `srv:agent/collect` 去领它。
    ///
    /// 判据跟本文件顶部一致：**只有 runner 自己知道**。detached 名单与 stash 都是
    /// [`crate::subtree::Subtree`] 的轮内局部表，core 里没有任何东西认识「后台子」
    /// 这个概念，所以它走不了 `Notice`。
    ///
    /// **052 借的是 [`RunnerEvent::TransportTrouble`]**（既有变体里唯一「一句话
    /// 文本、只进日志/面板、不参与任何判断」的口子），并在实做记录里诚实标注了
    /// 那个名字对不上语义——这不是传输故障，是编排失误。054 收掉那笔账。
    ///
    /// 归属（[`AgentEvent::agent`]）恒是**父**：「spawn 了后台子却没领」是父的
    /// 编排失误，告警该出现在父的时间线上；出事的那个子在 `child` 字段里，两者
    /// 不该挤在同一个位置上。
    ///
    /// 载荷是**事实**不是句子（[`OrphanFate`]）：措辞由看的人组，CLI 一份、web
    /// 一份，跟 `AgentActivity` 在 `agent-cli::print::agent_tree` 与
    /// `packages/web/src/render/agent_tree.ts` 各有一份呈现是同一条规矩。
    OrphanedChild { child: AgentId, fate: OrphanFate },

    /// 206：轮末这个 agent 的收件箱里还有 `count` 条 `Deliver::Now` 的话**没被
    /// 读到**——有人给它发了消息，而它在这一轮里再也没有组装过 provider 请求
    /// （多半是发的时候它已经答完了）。
    ///
    /// 这是**编排失误的信号，不是错误**：轮次结果照旧是它本来的样子，泵不会因此
    /// 多转一圈（决策 204 §二：一轮结束就是结束）。
    ///
    /// **`Deliver::NextTurn` 的条目不算在里面**，它们本来就该留到下一轮
    /// （206 §4 那条直觉陷阱：孤儿收尾「收件箱非空就告警」的写法会把正常情况
    /// 报成异常，接着有人会顺手清干净）。
    ///
    /// 载荷是**事实**不是句子（同 [`RunnerEvent::OrphanedChild`]）：措辞由看的人
    /// 组，CLI 一份、web 一份。
    UnreadMessages { agent: AgentId, count: usize },

    /// 211：**这一轮不是人开的，是留言自己开的**（决策 35 §二，自驱动轮次）。
    ///
    /// `remaining` 是扣掉这一轮之后**还剩几格**自驱动预算。两样都要给出去，
    /// 因为这是本仓第一次在没有用户输入的情况下继续消耗 token——用户失去的第一样
    /// 东西是「我知道现在在干什么」，所以「这一轮是自己开的」和「还能自己开几轮」
    /// 都不能只进日志。
    ///
    /// 归属恒是 **root**：自驱动的轮次只有 root 能开（`Deliver::NextTurn` 只投给
    /// root，子 agent 活不到下一轮）。
    AutoTurnStarted { remaining: u32 },

    /// 211：**有留言等着，但这一轮没有自己开**。`pending` 是收件箱里还剩几条。
    ///
    /// 三种成因（[`AutoTurnHold`]）都不是错误，但都必须说出来——**留言原地留着、
    /// 不丢弃**是这三条共有的承诺，而一个不说话的「什么都没发生」跟「留言被吞了」
    /// 在外面长得一模一样。
    AutoTurnHeld { pending: usize, reason: AutoTurnHold },

    /// 109：一份摘要被写进状态了（[`agent_core::Session::apply_summary`] 成功）
    /// ——压缩点在时间线上可见的信号。判据同本文件顶部：`upto` **只有 runner
    /// 自己知道**（105 定死 `Event::CompactDone` 不带它，`crate::compact_slot::
    /// CompactSlots` 才是唯一记着它的地方，见 `crate::compact_writeback` 模块
    /// 文档）。`turn_id` 是发生这次回写时的 [`agent_core::Session::turn_id`]
    /// ——UI 拿它跟 `undo`/`redo` 帧的 `turn_id` 对，一次 turn 粒度的撤销/重做
    /// 就能精确决定要不要连带撤回/恢复这条压缩标记，不必假设「一次 undo 正好
    /// 对应最近一条标记」（那个假设在压缩是异步产出、可能跨轮落地时不成立）。
    CompactionApplied {
        turn_id: u64,
        upto: usize,
        summary_id: SummaryId,
    },

    /// 109：一批工具调用结果被清除了（[`agent_core::Session::clear_tool_results`]
    /// 新清了至少一个，不是全部幂等命中）——被清的调用要能在时间线上标出来，
    /// 而不是凭空消失。`turn_id` 同 [`RunnerEvent::CompactionApplied`] 的理由。
    ToolResultsCleared {
        turn_id: u64,
        call_ids: Vec<ToolCallId>,
    },
}

/// 一个没人领的后台子 agent 在轮末是怎么收场的——[`crate::orphan::reap`] 的三条
/// 出路，一一对应。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrphanFate {
    /// 还活着 → 连同 `descendants` 个后代一起被 `Session::despawn_child` 拆掉。
    /// 它在飞的那次调用回来时会撞活性闸被丢（`crate::orphan` 模块文档 §砍尾）。
    Despawned { descendants: usize },
    /// 拆不掉（`agent_core::DespawnRefused`，比如子树之外还有读者）：状态一个
    /// 字节没改，它会以**活着**的状态留到下一轮。`reason` 是那个拒绝的可读描述。
    Kept { reason: String },
    /// 已经跑完，结果在「已完成未领取」stash 里躺到轮末没人领，`bytes` 字节被
    /// 丢弃。`is_error` 说的是**子自己**成没成，不是这次丢弃成没成。
    Discarded { bytes: usize, is_error: bool },
}

/// 为什么这一轮没有自己开（211）。载荷是**事实**不是句子，同 [`OrphanFate`]。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AutoTurnHold {
    /// 自驱动预算见底。**只有真实用户输入能把它加满**——按时间或按「有进展」自动
    /// 续期都是把闸接回被它约束的循环里，那就等于没有闸（决策 35 §五）。
    BudgetExhausted,
    /// 用户在自驱动跑到一半时喊了停。已经跑完的那几轮**不算失败**，剩下的留言
    /// 留在收件箱里等下一次真实驱动。
    Cancelled,
    /// 刚从崩溃里恢复出来。**恢复不自动开轮**，两条理由都不能让步：打开应用它
    /// 自己就开始烧钱；以及用户还没来得及看上一轮发生了什么。恢复是「回到现场」，
    /// 不是「接着跑」。
    Recovered,
}
