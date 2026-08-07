//! `CallProvider` 真正发请求的那个线程。ADAPTER.md §时序画的两条线
//! （「actor 线程」/「IO 线程」）在这里落成真的 `std::thread`：`encode`/
//! `check_drift` 已经在调用方（`provider_call`）的 actor 线程上做完，这个
//! 模块只管「把 `Encoded.body` 发出去、把流喂回来」。
//!
//! 拆成单独一个线程，012 时是为了让调用方能用 `recv_timeout` 实现一个 provider
//! 调用的整体超时；029 起它还是**并行的载体**：每个 agent 一个 IO 线程，子 agent
//! 的 provider 调用因此真的同时在飞（STATE-MODEL §「并发」：子 agent 的并发是
//! IO 并发，不是状态并发——回写全部串行过泵）。
//!
//! # 一条统一的 mpsc，消息自带 agent tag
//!
//! 029 之前每次调用各自一个 rendezvous channel，调用方就地 `recv_timeout` 等到
//! 底。多 agent 之后「等」的对象不再是一个 channel 而是**一批**在飞的调用，而
//! std 没有 select——所以改成所有 IO 线程往泵的同一个
//! `sync_channel(0)` 里发，provider 消息带上 `(agent, attempt)`。容量仍然是 0：
//! 一个线程发一条增量就等泵收走，天然背压不变。
//!
//! # 放弃一个在飞调用之后
//!
//! 泵在超时/取消时先锁存该调用独立的取消标志，再把凭据从在飞表划掉。请求准备会在
//! 每次上传前、两张图片之间和构造 chat body 前观察这枚不可复位的标志；已经进入
//! 稳定同步上传 API 的请求允许物理完成，但不会再开始下一张上传或 chat。晚到的
//! [`IoMsg`] 仍按 `(agent, attempt)` 找不到凭据而被丢弃；整轮结束后接收端被丢掉，
//! 发送端也会在下一次 `send` 收到 `Err`。

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::thread;

use agent_core::{AgentId, ToolCallId};
use agent_providers::StreamEvent;

use crate::event::RunnerEvent;
use crate::execution_binding::ExecutionBinding;
use crate::provider_attempt::ProviderAttemptId;
use crate::provider_message::ProviderMessage;

/// 这些线程往泵里发的东西。**两类生产者共用这一条泵 channel**：provider 调用的
/// IO 线程（本文件）和 MCP `tools/call` 的背景线程（`crate::mcp_call`），泵按各自
/// 的键认领在飞 credential 再落地（029 起就是「所有 IO 线程发同一个
/// `sync_channel(0)`，消息自带谁是谁」这一形状）。
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

/// 起线程发一次请求。**不返回 `JoinHandle`**——超时路径要能放弃这个线程而不
/// join 它（`provider_call` 模块文档的事故记录），给了 `JoinHandle` 只会诱使
/// 调用方去 join。
pub(crate) fn spawn(
    tx: SyncSender<IoMsg>,
    agent: AgentId,
    attempt: ProviderAttemptId,
    binding: ExecutionBinding,
    body: Vec<u8>,
    cancel_token: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        // 线程从这一刻起欠泵一条终态消息。正常路径由 `settle` 还上；panic 路径
        // 由 `Drop` 还上（`ProviderMessage::gone`）——两条路都还，泵因此永远不会为一个
        // 已经死掉的线程干等。
        let mut debt = DoneDebt {
            agent: agent.clone(),
            attempt,
            tx: tx.clone(),
            settled: false,
        };

        let mut acc = binding.provider.accumulator();
        let result = binding.client.post_stream(
            &binding.endpoint,
            &binding.api_key,
            &body,
            &cancel_token,
            |line| {
                for ev in acc.push_line(line) {
                    let Some(event) = translate(ev) else { continue };
                    let delta =
                        IoMsg::Provider(ProviderMessage::delta(agent.clone(), attempt, event));
                    if tx.send(delta).is_err() {
                        // 接收端没了：泵已经收工（或者已经放弃这次调用），没有理由
                        // 继续读下去。
                        return ControlFlow::Break(());
                    }
                }
                ControlFlow::Continue(())
            },
        );
        let (blocks, stop, usage) = acc.finish();
        debt.settle(ProviderMessage::done(
            agent.clone(),
            attempt,
            result,
            blocks,
            stop,
            usage,
        ));
    });
}

/// 「这个线程还欠泵一条终态消息」。panic 时发送 [`ProviderMessage::gone`]。
struct DoneDebt {
    agent: AgentId,
    attempt: ProviderAttemptId,
    tx: SyncSender<IoMsg>,
    settled: bool,
}

impl DoneDebt {
    fn settle(&mut self, msg: ProviderMessage) {
        self.settled = true;
        let _ = self.tx.send(IoMsg::Provider(msg));
    }
}

impl Drop for DoneDebt {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.tx.send(IoMsg::Provider(ProviderMessage::gone(
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
