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
//! `sync_channel(0)` 里发，消息带上自己是谁（[`IoMsg`] 每个变体第一个字段都是
//! `agent`）。容量仍然是 0：一个线程发一条增量就等泵收走，天然背压不变。
//!
//! # 放弃一个在飞调用之后
//!
//! 泵超时/取消时把这个调用从在飞表里划掉，**不 join、不断它的连接**
//! （`provider_call` 模块文档的事故记录）。它继续往共享 channel 里发的东西，
//! 泵按 (agent, epoch) 认不出来就丢——跟 `Session::step` 对过期 epoch 的处理
//! 同一条判据：过期回执是正常现象，不是错误。整轮结束后泵连同接收端一起丢掉，
//! 这些线程下一次 `send` 立刻拿到 `Err`，那才是它们收手的信号
//! （下面 `on_line` 里的 `ControlFlow::Break`）。

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::SyncSender;
use std::thread;

use agent_core::{AgentId, ContentBlock, StopReason, TokenUsage, ToolCallId};
use agent_providers::{Provider, StreamEvent};
use agent_transport::{Client, StreamOutcome, TransportError};

use crate::event::{AgentEvent, RunnerEvent};

/// 这些线程往泵里发的东西。**两类生产者共用这一条泵 channel**：provider 调用的
/// IO 线程（本文件）和 MCP `tools/call` 的背景线程（`crate::mcp_call`），泵按各自
/// 的键认领在飞 credential 再落地（029 起就是「所有 IO 线程发同一个
/// `sync_channel(0)`，消息自带谁是谁」这一形状）。
pub(crate) enum IoMsg {
    /// 中途的增量，已经带好归属（`AgentEvent`）。
    Delta(AgentEvent),
    /// 流到头之后的终态一次性打包。
    Done {
        agent: AgentId,
        result: Result<StreamOutcome, TransportError>,
        blocks: Vec<ContentBlock>,
        stop: StopReason,
        usage: TokenUsage,
    },
    /// 这个线程没留下 [`IoMsg::Done`] 就没了（它 panic 了）。
    ///
    /// 029 之前这件事由「per-call channel 的发送端被 drop → 调用方 `recv` 拿到
    /// `Disconnected`」自然表达；换成一条共享 channel 之后发送端永远还在别的线程
    /// 手里，那个信号消失了，只能显式补一条——否则一个 panic 掉的 IO 线程会让
    /// 它那个 agent 一直挂到超时预算耗尽，把一个即刻可判的 bug 拖成 120 秒。
    Gone { agent: AgentId },
    /// 一次在飞的 MCP `tools/call` 报回结果（043）。`content`/`is_error` 已由
    /// `agent-mcp::flatten_tool_result` 从 wire 拍平——泵这边只按 `(agent, call_id)`
    /// 认领 `crate::mcp_call::McpCall` credential，epoch 由 credential 提供、回写前
    /// 过 `Session::step` 的 epoch 闸（红线 6）。跟 provider 的 `Done` 是两类东西
    /// （一个工具结果、一个模型响应），所以是独立变体、按不同键落地。
    McpDone { agent: AgentId, call_id: ToolCallId, content: Arc<str>, is_error: bool },
}

/// 起线程发一次请求。**不返回 `JoinHandle`**——超时路径要能放弃这个线程而不
/// join 它（`provider_call` 模块文档的事故记录），给了 `JoinHandle` 只会诱使
/// 调用方去 join。
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn(
    tx: SyncSender<IoMsg>,
    agent: AgentId,
    client: Arc<Client>,
    provider: Arc<dyn Provider>,
    endpoint: String,
    api_key: String,
    body: Vec<u8>,
    cancel: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        // 线程从这一刻起欠泵一条终态消息。正常路径由 `settle` 还上；panic 路径
        // 由 `Drop` 还上（`IoMsg::Gone`）——两条路都还，泵因此永远不会为一个
        // 已经死掉的线程干等。
        let mut debt = DoneDebt { agent: agent.clone(), tx: tx.clone(), settled: false };

        let mut acc = provider.accumulator();
        let result = client.post_stream(&endpoint, &api_key, &body, &cancel, |line| {
            for ev in acc.push_line(line) {
                let Some(event) = translate(ev) else { continue };
                let delta = IoMsg::Delta(AgentEvent { agent: agent.clone(), event });
                if tx.send(delta).is_err() {
                    // 接收端没了：泵已经收工（或者已经放弃这次调用），没有理由
                    // 继续读下去。
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        });
        let (blocks, stop, usage) = acc.finish();
        debt.settle(IoMsg::Done { agent: agent.clone(), result, blocks, stop, usage });
    });
}

/// 「这个线程还欠泵一条终态消息」。见 [`IoMsg::Gone`]。
struct DoneDebt {
    agent: AgentId,
    tx: SyncSender<IoMsg>,
    settled: bool,
}

impl DoneDebt {
    fn settle(&mut self, msg: IoMsg) {
        self.settled = true;
        let _ = self.tx.send(msg);
    }
}

impl Drop for DoneDebt {
    fn drop(&mut self) {
        if !self.settled {
            let _ = self.tx.send(IoMsg::Gone { agent: self.agent.clone() });
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
