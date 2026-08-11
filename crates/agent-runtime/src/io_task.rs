//! 一次 provider 调用的 IO 载体：**一个 future**（117 之前是一个
//! `std::thread`，见本文件的 git 历史 `io_thread.rs`）。
//!
//! ADAPTER.md §时序画的两条线（「actor 线程」/「IO 线程」）从这一版起是「泵所在
//! 的那一个线程」与「[`crate::io_stream`] 底下那条只读字节的工作线程」；
//! `encode`/`check_drift` 仍然在调用方（`provider_call`）做完，这个模块只管
//! 「把请求发出去（委托给行源）、把流累积成消息喂回泵」。
//!
//! # `io_thread::spawn` 那一下扛的四件事，现在各归各家
//!
//! | 它扛的 | 117 之后归谁 |
//! |---|---|
//! | 发请求并回喂流 | [`crate::io_stream`]（平台接缝；native 底下还有一条只读字节的工作线程，wasm 上是 `fetch`） |
//! | **029 的并行载体** | [`crate::io_bus`] 的 `FuturesUnordered`：同一个事件循环上并发跑的一批 future |
//! | **会合背压** | 本文件的 `delta_tx.send(..).await`：`futures` 的 `mpsc::channel(0)`，每个发送端最多攒一条 |
//! | **超时后放弃而不 join** | future 被丢掉即可；欠的那条终态消息由 [`DoneDebt`] 的 `Drop` 还上 |
//!
//! # 一条统一的 mpsc，消息自带 agent tag（029 的形状没变）
//!
//! 多 agent 之后「等」的对象是**一批**在飞调用，所以所有 IO 载体往泵的同一个
//! channel 里发、provider 消息带上 `(agent, attempt)`。换成 `futures` 的 channel
//! 之后这条一字未改，改的只有「怎么等」和「谁在发」。
//!
//! # 放弃一个在飞调用之后
//!
//! 泵在超时/取消时先锁存该调用独立的取消标志，再把凭据从在飞表划掉。请求准备会
//! 在每次上传前、两张图片之间和构造 chat body 前观察这枚不可复位的标志；已经进入
//! 稳定同步上传 API 的请求允许物理完成，但不会再开始下一张上传或 chat。晚到的
//! [`IoMsg`] 仍按 `(agent, attempt)` 找不到凭据而被丢弃——**这条是 117 的头等大事**：
//! 换成有缓冲的 channel 之后，「泵划掉凭据」与「发送端手上那条已经写进 channel 的
//! 增量」之间第一次出现了时间窗，对抗测试见 `io_task_tests.rs` 的
//! `a_delta_already_in_the_channel_is_dropped_once_its_credential_is_gone`。

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agent_core::{AgentId, ToolCallId};
use agent_providers::{StreamAccumulator, StreamEvent};
use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};

use crate::event::RunnerEvent;
use crate::execution_binding::ExecutionBinding;
use crate::image_materialization::ProviderRequest;
use crate::io_stream::{self, StreamItem};
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_message::ProviderMessage;

/// 这些载体往泵里发的东西。**两类生产者共用这一条泵 channel**：provider 调用的
/// IO future（本文件）和 MCP `tools/call` 的背景线程（`crate::mcp_call`，决策 26
/// 之下 native 独有，照旧用线程），泵按各自的键认领在飞 credential 再落地。
pub(crate) enum IoMsg {
    /// Provider deltas and terminal outcomes, all correlated by `(agent, attempt)`.
    Provider(ProviderMessage),
    /// 一次在飞的 MCP `tools/call` 报回结果（043）。`content`/`is_error` 已由
    /// `agent-mcp::flatten_tool_result` 从 wire 拍平——泵这边只按 `(agent, call_id)`
    /// 认领 `crate::mcp_call::McpCall` credential，epoch 由 credential 提供、回写前
    /// 过 `Session::step` 的 epoch 闸（红线 6）。跟 provider 的 `Done` 是两类东西
    /// （一个工具结果、一个模型响应），所以是独立变体、按不同键落地。
    McpDone {
        agent: AgentId,
        call_id: ToolCallId,
        content: Arc<str>,
        is_error: bool,
    },
}

/// 泵 channel 的发送端。**每个持有者都该握自己的一份 clone**——`futures` 的
/// 有界 channel 是「buffer + 每个 sender 一个保底槽位」，槽位是按 sender 记的，
/// 见 [`DoneDebt`] 为什么必须独占一个。
pub(crate) type IoSender = mpsc::Sender<IoMsg>;

/// 造一次 provider 调用的 IO future。**不返回任何句柄**——超时路径要能放弃它而
/// 不等它（`provider_call` 模块文档的事故记录），给了句柄只会诱使调用方去等。
///
/// `delta_tx` 与 `debt_tx` 必须是**两份独立的 clone**：前者用来逐条发增量（会
/// 被背压停住），后者只发一条终态消息。理由见 [`DoneDebt`]。
pub(crate) fn task(
    delta_tx: IoSender,
    debt_tx: IoSender,
    agent: AgentId,
    attempt: ProviderAttemptId,
    binding: ExecutionBinding,
    request: ProviderRequest,
    cancel_token: Arc<AtomicBool>,
) -> impl Future<Output = ()> + 'static {
    // 累积器留在泵这一侧（117 之前它跑在 IO 线程上）：这样工作线程只剩「字节 →
    // 行」，wasm 上换掉行源之后累积逻辑一行都不用动。
    let accumulator = binding.provider.accumulator();
    let items = io_stream::open(binding, request, cancel_token);
    run(delta_tx, debt_tx, agent, attempt, accumulator, items)
}

/// 载体本体，**与平台无关**：吃一串 [`StreamItem`]，吐 `(agent, attempt)` 信封
/// 里的增量与终态。行源换成 `fetch` 也好、换成测试里手工喂的 channel 也好，这
/// 段逻辑都是同一份——`io_task_tests.rs` 正是靠这一点在没有任何 HTTP 的情况下
/// 把「欠债—还债」和「幽灵增量」两条焊死。
pub(crate) async fn run(
    mut delta_tx: IoSender,
    debt_tx: IoSender,
    agent: AgentId,
    attempt: ProviderAttemptId,
    mut accumulator: StreamAccumulator,
    mut items: mpsc::Receiver<StreamItem>,
) {
    // 这个 future 从这一刻起欠泵一条终态消息。正常路径由 `settle` 还上；被丢掉
    // （超时/取消/整轮收工）或行源半路没了由 `Drop` 还上——两条路都还，泵因此
    // 永远不会为一个已经不存在的调用干等。
    let mut debt = DoneDebt {
        agent: agent.clone(),
        attempt,
        tx: debt_tx,
        settled: false,
    };
    let mut private_references = Vec::new();

    while let Some(item) = items.next().await {
        match item {
            StreamItem::Prepared(references) => private_references = references,
            StreamItem::PreparationFailed(failure) => {
                debt.settle(ProviderMessage::preparation_failed(
                    agent.clone(),
                    attempt,
                    failure,
                ));
                return;
            }
            StreamItem::Line(line) => {
                for ev in accumulator.push_line(&line) {
                    let Some(event) = translate(ev) else { continue };
                    let delta =
                        IoMsg::Provider(ProviderMessage::delta(agent.clone(), attempt, event));
                    // **这一句就是会合背压**：泵没把上一条取走之前，这个 future
                    // 停在这里（不是自旋、不是攒在内存里）。115 拍板接受的
                    // 「每个发送端最多缓冲 1 条」，缓冲的就是它。
                    if delta_tx.send(delta).await.is_err() {
                        // 接收端没了：泵已经收工，债也没人收了。
                        return;
                    }
                }
            }
            StreamItem::Done(result) => {
                let (blocks, stop, usage) = accumulator.finish();
                debt.settle(ProviderMessage::done(
                    agent.clone(),
                    attempt,
                    result,
                    blocks,
                    stop,
                    usage,
                    std::mem::take(&mut private_references),
                ));
                return;
            }
        }
    }
    // 行源没留下终态就断了（工作线程 panic）——落到这里，`debt` 的 `Drop` 还上
    // 一条 `gone`，泵把它翻成一次可重试的失败，不挂住。
}

/// 「这个 future 还欠泵一条终态消息」。被丢掉时发 [`ProviderMessage::gone`]。
///
/// # 为什么它必须独占一份 sender
///
/// `Drop` 里没有 `.await`，只能 `try_send`。`futures` 的有界 channel 容量是
/// `buffer + sender 数`——**每个 sender 保底一个槽位**，一个 sender 塞满自己那
/// 个槽之后就被 park，直到接收端取走才能再发。所以只要这份 clone **一辈子只发
/// 一条消息**（`settle` 与 `Drop` 二选一，`settled` 闩保证），它的 `try_send`
/// 就只可能因为「接收端没了」而失败，绝不会因为「channel 满了」而失败——那正是
/// 会把「对话永久转圈」这类事故放进来的口子。这条不是推理，`io_task_tests.rs`
/// 的 `a_fresh_sender_always_has_one_slot_even_when_the_channel_is_full` 把它
/// 焊在测试里。
///
/// 顺带：增量走的是另一份 clone，两条消息进的是同一条 FIFO 队列，所以终态永远
/// 排在此前所有增量之后，换 channel 没有引入乱序。
struct DoneDebt {
    agent: AgentId,
    attempt: ProviderAttemptId,
    tx: IoSender,
    settled: bool,
}

impl DoneDebt {
    fn settle(&mut self, msg: ProviderMessage) {
        self.settled = true;
        let _ = self.tx.try_send(IoMsg::Provider(msg));
    }
}

impl Drop for DoneDebt {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.tx.try_send(IoMsg::Provider(ProviderMessage::gone(
                self.agent.clone(),
                self.attempt,
            )));
        }
    }
}

/// 只转发「宿主自己看不见」的三种增量（001 判断 1 的判据延续到这里）；
/// `Finished`/`UsageReady`/`Done` 是累积器内部记账用的，不是给人看的东西。
fn translate(ev: StreamEvent) -> Option<RunnerEvent> {
    match ev {
        StreamEvent::TextDelta(t) => Some(RunnerEvent::TextDelta(t)),
        StreamEvent::ThinkingDelta(t) => Some(RunnerEvent::ThinkingDelta(t)),
        StreamEvent::ToolCallStarted { name, .. } => Some(RunnerEvent::ToolCallStarted { name }),
        StreamEvent::Finished(_) | StreamEvent::UsageReady(_) | StreamEvent::Done => None,
    }
}

#[cfg(test)]
#[path = "io_task_tests.rs"]
mod tests;
