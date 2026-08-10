//! `Effect::Emit` 的载荷：loop 要说给人听的话。
//!
//! 判据只有一个——**宿主自己看不见的，才在这里**。宿主（012 的 runner）手里已经有
//! 流式增量、`Encoded` 的 drift / predicted_cache、响应的 usage 和 adjustments，
//! 那些它直接打就是了（022 的 CLI 现在就是这么打的），再绕一圈进 core 又发回来，
//! 是白走一趟还多一份要维护的形状。
//!
//! 反过来，014 的注意条写死了另一半：「如果为了显示某个东西不得不去掏 loop 的内脏，
//! 说明事件契约漏了一种事件」。所以只有 core 自己知道的事实要在这里出现。
//!
//! M1 三条，各自的消费者写在变体注释里；第三条（`ProtocolViolation`）是 002 加的，
//! 见它自己的文档注释。105 加的两条压缩通报也过了同一道判据，理由写在变体上：
//! **闸放行没放行，只有 core 知道**。

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::ToolCallId;

use super::state::TurnStatus;

/// 一条通报。**通报不是命令**：宿主可以只打印、可以推 SSE、可以丢掉，loop 的正确性
/// 不依赖它被消费。
///
/// 里面没有 agent 归属：M1 是单 agent，CLI 打印时无处可用。M3 多 agent 并行输出时
/// 要能分辨「谁说的」，那时加（issue 006 定了子 agent 形态之后）。
///
/// 032：`SessionEvent::Notice` 的载荷，`ts` feature 门后面导出 TS。**`Notice`
/// 本身仍然不带 agent 归属**——上一段写死了「里面没有 agent 归属」，这条判断
/// 034 也没有理由推翻：多 agent 并行输出时「谁说的」这件事该由 `agent-server`
/// 的 `Frame { agent, event }` 信封在 SSE 帧那一层统一携带（`SessionEvent` 的
/// 每个变体都套在 `Frame` 里，`Notice` 只是 `SessionEvent::Notice` 的载荷，不必
/// 也不该单独再背一份 agent 字段）。`AgentId` 因此仍然没有从这个变体触达协议面
/// ——它是从 `Frame.agent` 那条路径触达的（见 `AgentId` 自己的类型文档），跟
/// `Notice` 无关。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum Notice {
    /// 轮状态变了。
    ///
    /// **消费者**：012 的 runner——它靠 [`TurnStatus::is_terminal`] 知道该停止驱动，
    /// 这是 loop 说「停」的唯一出口（effect 列表为空不代表结束，可能只是在等在飞的
    /// 结果，也可能是一个过期事件被闸挡了）；014 据此打印本轮怎么结束的，
    /// `Done { truncated }` 正好是 016 验收要的「答完了 vs 被截断了」。
    TurnStatusChanged { status: TurnStatus },

    /// 一次工具输出在进消息历史之前被截断了（决策 19 / issue 004，
    /// 实现是 [`crate::limits::truncate_tool_output`]）。
    ///
    /// **消费者**：014 的验收要求「工具调用要可见：调了什么、参数是什么、**结果多长**」。
    /// 前两件宿主从 `Effect::ExecuteTool` 就知道，原始长度它也知道——但**截断发生在
    /// core 边界**（executor 不知道 prompt 预算），所以「模型实际看到了多少」只有
    /// core 说得出。这条不打出来，人看到的「结果 10MB」和模型看到的 32KiB 是两回事，
    /// 而模型基于残缺数据下结论时你会先怀疑模型。
    ToolOutputTruncated {
        call_id: ToolCallId,
        /// 截断前的字节数。
        original_bytes: u64,
        /// 实际进 prompt 的内容字节数（不含截断标记）。
        kept_bytes: u64,
    },

    /// 转移表（002）拒绝了一次转移：要么是 (状态, 事件) 组合本身没有意义
    /// （比如 `Idle` 收到 `ToolResult`、`Done` 收到 `UserInput`），要么事件的
    /// 内容跟它自己声明的字段矛盾（比如 `stop: ToolUse` 却一个 `ToolUse` 块
    /// 都没有）。
    ///
    /// **002 加的唯一一个新变体**，判据沿用 001 定的那条：「宿主自己看不见的
    /// 才在这里」——非法转移这件事只有 core 看得见（宿主只是转发了一个事件，
    /// 它不知道这个事件在 core 眼里跟当前状态对不上）。002 的验收原文是
    /// 「非法转移是显式错误，不是静默留在原状态」：状态确实不变，但这条通报
    /// 让「不变」是可观测的，不是「什么都没发生」。
    ///
    /// `event` 是 `format!("{event:?}")` 而不是一个结构化的 `EventKind` 枚举：
    /// 这条通报的消费者是人和日志（014 打印、012 决定要不要升级成致命错误），
    /// 不是要拿它做判断的程序逻辑，不值得为它专门加一个可序列化的新公开类型。
    ProtocolViolation { state: TurnStatus, event: Arc<str> },

    /// 一次 `ProviderFailed`（或 provider `Timeout`）被判定为可重试，且重试预算
    /// 没耗尽——core 决定再发一次 `CallProvider`。001 把这条通报的形状留白
    /// （「重试几次、按哪些 `ErrorClass` 重试都是 016 的设计，现在定只能猜字段」），
    /// 016 落地时填上。
    ///
    /// `attempt` 是这是第几次重试（从 1 起，即这次决定重试后 `retries_used` 的
    /// 新值）；`max_retries` 是这一条重试链的预算上限——两个数字拼起来就是
    /// 「重试中 (attempt/max_retries)」。**退避的节奏不在这里**：那是 transport
    /// 的事（红线 7），这条通报只报「决定重试了」这个事实本身。
    ///
    /// **消费者**：014 打「重试中」提示。
    Retrying { attempt: u32, max_retries: u32 },

    /// 一份摘要**被这一代接受了**（105）。
    ///
    /// 判据仍然是那一条「宿主自己看不见的才在这里」：宿主当然知道它送回了一份摘要，
    /// 它不知道的是**core 收没收**——`Effect::Compact` 在飞期间用户可能取消或 undo
    /// 过，那样的回执会被 `Session::step` 的闸静默丢弃（丢弃**不**通报，理由见
    /// `command/step.rs`：过期回执是正常现象，每条都喊一声只会刷屏）。于是「接受」
    /// 这一侧必须说话，否则红线 6 那道闸的两种结果在外面完全一样，
    /// 一个「不管 epoch 一律丢弃」的实现跟正确实现分不出来。
    ///
    /// 没有字段：这条通报的全部内容就是「闸放行了」。摘要多少字节宿主自己知道，
    /// 盖住了哪一段要等 107 真的写状态之后才有答案（那时它是**状态**不是通报——
    /// 事后任何时候问都该答得出，`TurnStatus` 的类型文档定过同一条判据）。
    /// 也没有 `agent`：整个 [`Notice`] 都没有，归属由 `Frame.agent` 那一层带
    /// （见本类型的文档注释）。
    ///
    /// **消费者**：109（时间线上标出压缩点）；CLI 打一行小字。
    CompactionSummaryReceived,

    /// 这一次压缩没做成（105）。
    ///
    /// **正常事件不是异常路径**（106 验收）：压缩这一次作废，边界不动，下一轮照常跑
    /// ——所以它是一条通报，不是 `Failure`、也不进 `TurnStatus`。人该看得见
    /// 「刚才那次压缩没成」，否则下一轮又全价重编码一遍时没人知道为什么。
    ///
    /// 跟 [`Notice::CompactionSummaryReceived`] 分两个变体而不是共用一个
    /// `{ ok: bool }`：`ToolResult`/`ToolFailed` 分家时定过这条规矩，
    /// 一个布尔标志会逼每个消费者现场把它翻译回两句话。
    CompactionFailed,
}

// 想过但**没有**定的通报，以及为什么：
//
// - 缓存兜底的三层告警（024）：判读的输入（drift / predicted_cache / usage）宿主
//   全部持有，第 1 层甚至发生在**请求发出之前**——那时 loop 根本没被 step 过，
//   走 Emit 出不来。024 自己定它的告警形状。
// - 「模型说了什么」：流式增量归宿主直接打（accumulator 在宿主那边，ADAPTER.md
//   §时序），完整消息进 `TurnState::messages`。core 不重复发一遍文本。
// - 「工具开始执行了」：`Effect::ExecuteTool` 本身就是宿主收到的东西，再通报一次
//   是同一件事说两遍。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_variants() {
        let notices = vec![
            Notice::TurnStatusChanged {
                status: TurnStatus::Thinking,
            },
            Notice::TurnStatusChanged {
                status: TurnStatus::Done { truncated: true },
            },
            Notice::ToolOutputTruncated {
                call_id: ToolCallId::new("call_1"),
                original_bytes: 10 * 1024 * 1024,
                kept_bytes: 32 * 1024,
            },
            Notice::ProtocolViolation {
                state: TurnStatus::Idle,
                event: Arc::from("ToolResult { .. }"),
            },
            Notice::Retrying {
                attempt: 1,
                max_retries: 2,
            },
            Notice::CompactionSummaryReceived,
            Notice::CompactionFailed,
        ];
        let s = serde_json::to_string(&notices).unwrap();
        assert_eq!(serde_json::from_str::<Vec<Notice>>(&s).unwrap(), notices);
    }
}
