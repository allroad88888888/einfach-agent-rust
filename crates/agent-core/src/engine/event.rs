//! 喂进 loop 的事件：外面世界发生的事实。
//!
//! 事件是**已经发生的**，不是请求。宿主在 actor 线程上把它交给
//! [`crate::engine::step`]，core 据此推进状态、产出下一批 effect。
//!
//! ## 两件由这个 issue 裁决的事
//!
//! **一、`ProviderDone` 带 `adjustments`**（决策 17）。core 不事前问「你能不能强制
//! 指定工具」，只在事后知道「它降级了」。调整在 `encode` 时就产生（降不降级组装时就
//! 知道，不用等响应），宿主把它随这条事件喂进来。这是红线 12 在事件层的落点：
//! 能力位不过接缝，调整过。
//!
//! **二、流式增量不是事件。** issue 001 的清单里有 `ProviderChunk`，012 也写着
//! 「流式增量转成 `ProviderChunk` 事件」——这里推翻它，理由三条：
//!
//! 1. **它不改变任何状态、不产出任何 effect。** 累积器活在宿主那边
//!    （ADAPTER.md §时序：宿主喂行给 `accumulator`），`Message` 的文档注释又写死了
//!    「只放完成的消息，流式中间态不进这里」。喂进 core 的每个 chunk 只能原样弹回一条
//!    `Emit`，一轮几千次空转往返。
//! 2. **它会在 002 的穷举表里凿一个洞。** 002 的验收是「每个 (状态, 事件) 组合都有
//!    明确结果，**没有隐式的「忽略」**」。一个天生什么都不做的事件，在每一行都只能
//!    写「忽略」——那不是转移表，那是给转移表开了个后门。
//! 3. **打印路径不需要它。** 014 要的「流式增量实时打印」在宿主的流回调里就能做
//!    （022 的 CLI 已经在做），走 core 反而多一跳延迟。
//!
//! 反过来说，什么时候该翻案：如果哪天 core 需要**基于半截生成做决策**（比如看到
//! 前几个 token 就抢跑取消），那时 chunk 才第一次有转移可写，那时再加。
//!
//! ## 推迟的事件
//!
//! | 推迟的 | 等谁 | 为什么 |
//! |---|---|---|
//! | `Undo` / `Redo` | issue 017（M2） | M1 没有 store、没有 undo log，`Undo` 事件唯一能做的事（bump epoch）没有任何东西回滚，是空转 |
//! | `ChildFinished` | issue 006（M3） | 没有子 agent。等待子 agent 完成在 M2 之后是一个 derived atom（`Pending` 沿依赖图汇聚），未必长成一个事件 |

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::ids::{AgentId, ToolCallId};
use crate::seam::{Adjustment, ErrorClass, PrefixImage};
use crate::value::message::ContentBlock;
use crate::value::session::{StopReason, TokenUsage};

use super::epoch::Epoch;

/// 外面发生的一件事。
///
/// 带 `epoch` 的是**在飞 effect 的回执**，要过闸（红线 6）；不带的是用户意图，
/// 用户永远针对当前世界说话，过闸只会把他刚按下的那一下丢掉。
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Event {
    /// 用户说了一句话。
    ///
    /// 只带文本不带 `Message`：`MessageId` 的铸造归 core（历史活在
    /// [`crate::engine::TurnState`] 里，工具结果那条消息本来也是 core 造的），
    /// 让宿主造一半消息会出现两个铸号者。确定性铸号规则本身留给 009/010
    /// （见 `MessageId` 的注释）。
    ///
    /// 不带 epoch：用户意图针对的永远是当前世界。
    UserInput {
        agent: AgentId,
        text: Arc<str>,
    },

    /// provider 这一轮回完了（宿主已经把流收完、accumulator 已经 `finish`）。
    ///
    /// `blocks` / `stop` / `usage` 就是 `Decoded` 的三件套，`adjustments` 是决策 17
    /// 的事后报告，`prefix` 是这次请求的前缀镜像——core **只存不判读**，下一轮原样
    /// 回填进 `Ingredients::prev_prefix`（ADAPTER.md：哪一段漂了只有 adapter 算得出）。
    ///
    /// `PrefixImage::prompt_tokens` 由存的时候用 `usage.prompt` 回填：那是纯赋值，
    /// 不是模型相关判断。
    ProviderDone {
        agent: AgentId,
        epoch: Epoch,
        blocks: Vec<ContentBlock>,
        stop: StopReason,
        usage: TokenUsage,
        prefix: PrefixImage,
        adjustments: Vec<Adjustment>,
    },

    /// provider 这一轮失败了。
    ///
    /// `class` 是 adapter `classify` 的产物，016 的错误分流按它转移——**core 不自己
    /// 写一套状态码判断**（红线 12：各家状态码分配不一致）。`message` 只进日志和
    /// CLI 输出，不参与任何判断。
    ProviderFailed {
        agent: AgentId,
        epoch: Epoch,
        class: ErrorClass,
        message: Arc<str>,
    },

    /// 一个工具执行成功了。`content` 是**未截断的原始输出**——截断在 core 边界做
    /// （决策 19：executor 不该知道 prompt 预算），所以宿主原样送进来。
    ToolResult {
        agent: AgentId,
        epoch: Epoch,
        call_id: ToolCallId,
        content: Arc<str>,
    },

    /// 一个工具执行失败了。
    ///
    /// 跟 [`Event::ToolResult`] 分成两个事件而不是共用一个 `is_error` 布尔：宿主那边
    /// 这本来就是两条路径（`Ok` / `Err`），合并会逼它现造一个「错误内容」字符串再标
    /// 一个 flag。**core 这边两者殊途同归**：都变成一条 `ContentBlock::ToolResult`
    /// 喂回模型，失败的那条 `is_error: true`（003：部分失败不中止，模型比我们更知道
    /// 这个失败要不要紧）。
    ToolFailed {
        agent: AgentId,
        epoch: Epoch,
        call_id: ToolCallId,
        error: Arc<str>,
    },

    /// 一个在飞的东西超时了。
    ///
    /// **计时器活在宿主**（012 的 runner），core 里没有 `Instant::now()`——这是
    /// 001 的验收原文，也是「测试能在零时间内模拟任意超时序列」（005）的前提。
    ///
    /// `call_id` 为 `None` 表示超时的是 provider 调用，`Some(id)` 表示是那个工具。
    /// 两者的转移不同（前者是可重试的失败，后者只让一个槽位落地），所以必须分得出。
    Timeout {
        agent: AgentId,
        epoch: Epoch,
        call_id: Option<ToolCallId>,
    },

    /// 用户要取消当前这一轮。
    ///
    /// 不带 epoch，**因为它是 bump epoch 的那一方**：用户按 Ctrl-C 针对的是「现在
    /// 在飞的一切」，带上一个 epoch 反而会出现「取消一个已经过期的世代」这种没有
    /// 意义的语义。
    Cancel { agent: AgentId },
}

impl Event {
    /// 这件事发生在**哪个 agent 头上**。
    ///
    /// 028 起这个字段真正路由：`Session::step` 拿它决定这一步写谁的槽位
    /// （每个 agent 的轮状态独立）。M1/M2 单 agent 时它恒等于 root，字段一直在
    /// （001 就定了），只是没有第二个取值。
    ///
    /// **路由权没有交给宿主**：`Session::step` 拿到它之后还要过一道活性闸——
    /// 不在本会话活名单上的 agent，事件直接丢弃。宿主说得出「这是替谁做的」，
    /// 说不了「这个 agent 存在」。
    ///
    /// 提取器写在这里而不是让 `step` 自己 `match`，理由与 [`Event::epoch`] 相同：
    /// 加事件变体时编译器在这个 `match` 上逼你回答「它是谁的」。
    pub fn agent(&self) -> &AgentId {
        match self {
            Event::UserInput { agent, .. }
            | Event::ProviderDone { agent, .. }
            | Event::ProviderFailed { agent, .. }
            | Event::ToolResult { agent, .. }
            | Event::ToolFailed { agent, .. }
            | Event::Timeout { agent, .. }
            | Event::Cancel { agent } => agent,
        }
    }

    /// 这个事件属于哪个世代。`None` = 用户意图，不过闸（红线 6）。
    ///
    /// 提取器写在这里而不是让 [`crate::engine::step`] 自己 `match`：闸是一处，
    /// 加事件变体时编译器会在这个 `match` 上逼你回答「它要不要过闸」，
    /// 而漏掉过闸就是幽灵结果——不报错、偶发、依赖时序。
    pub fn epoch(&self) -> Option<Epoch> {
        match self {
            Event::UserInput { .. } | Event::Cancel { .. } => None,
            Event::ProviderDone { epoch, .. }
            | Event::ProviderFailed { epoch, .. }
            | Event::ToolResult { epoch, .. }
            | Event::ToolFailed { epoch, .. }
            | Event::Timeout { epoch, .. } => Some(*epoch),
        }
    }
}
