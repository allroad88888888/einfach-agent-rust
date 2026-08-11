//! 泵与 IO 之间的那一层：一条 [`IoMsg`] 的 channel、一批**并发在跑**的 IO
//! future，以及「等下一条消息，但最多等这么久」这一个动作。
//!
//! 117 之前这三样东西是散的：channel 是 `runner.rs` 里一句
//! `mpsc::sync_channel(0)`，并发靠每个调用一条 `std::thread`，等待是
//! `rx.recv_timeout`。换掉载体之后它们变成了同一件事的三个面，所以收进一个类型。
//!
//! # 029 的并行现在长什么样
//!
//! 「子 agent 的并发**就是** IO 并发」（STATE-MODEL §并发）这句话，117 之前落
//! 在「每个 agent 一条 IO 线程」上，现在落在 [`FuturesUnordered`] 上：
//! [`IoBus::receive`] 每次被 poll 都会先把**所有**在飞的 IO future 推一遍
//! （`drive`），谁有进展谁往 channel 里写。退化成串行的写法（比如一次只 poll
//! 一个、或者 `await` 单个调用到底）**不会报错，只会变慢**——所以它有一条盯着
//! 「两个子 agent 的增量真的交替出现」的验收测试
//! （`tests/it/spawn_parallel_futures_interleave.rs`），不是靠观感。
//!
//! # 容量 0 不是会合，这是 115 拍板接受的
//!
//! `futures` 的有界 channel 容量是 `buffer + sender 数`：**每个 sender 保底一个
//! 槽位**，所以 `channel(0)` 也不是 `std::sync::mpsc::sync_channel(0)` 那种真会
//! 合（tokio 更干脆，`channel(0)` 直接 panic）。115 决策 3 拍板接受「每个发送端
//! 至少缓冲 1 条」，理由是在飞调用数本来就有上限（决策 20：深度 ≤3、子数 ≤8），
//! 所以缓冲总量有界、不是无限堆积；代价是**取消之后可能漏出一条幽灵增量**，那
//! 正好撞红线 6，所以必须有对抗测试证明它被 `(agent, attempt)` 挡掉——
//! `io_task_tests.rs` 与 `tests/it/late_provider_reply_after_timeout.rs` 两条。
//!
//! # 为什么泵自己也握一份发送端
//!
//! 跟 117 之前一样：`rx` 因此永远不会因为「所有发送端都没了」而结束——在飞与否
//! 由泵的在飞表回答，不是由 channel 的连接状态回答。

use std::future::{Future, poll_fn};
use std::task::{Context, Poll};
use std::time::Duration;
// 时钟走 `web-time`（114b）：native 上它就是 `pub use std::time::*`，wasm 上走
// `performance.now()`。这行如果退回 `std::time::Instant`，**编译照样过，wasm 上
// 第一次泵循环就 panic**——`wasm32-unknown-unknown` 没有时钟源。
use web_time::Instant;

use futures_channel::mpsc;
use futures_util::StreamExt;
use futures_util::future::{FutureExt, LocalBoxFuture};
use futures_util::stream::FuturesUnordered;

use crate::heartbeat::Heartbeat;
use crate::io_task::{IoMsg, IoSender};

pub(crate) struct IoBus {
    /// 泵自己那一份，见模块文档最后一节。
    tx: IoSender,
    rx: mpsc::Receiver<IoMsg>,
    /// 在飞的 IO future。`LocalBoxFuture`（不是 `BoxFuture`）是有意的：这些
    /// future 只在泵这一个线程上被 poll，不需要 `Send`——wasm 上更是压根没有
    /// 别的线程可以送过去。
    tasks: FuturesUnordered<LocalBoxFuture<'static, ()>>,
    heartbeat: Heartbeat,
}

impl IoBus {
    /// `tick` 是「什么都没发生也要醒一次」的节奏，见 [`crate::heartbeat`]。
    pub(crate) fn new(tick: Duration) -> Self {
        let (tx, rx) = mpsc::channel::<IoMsg>(0);
        IoBus {
            tx,
            rx,
            tasks: FuturesUnordered::new(),
            heartbeat: Heartbeat::start(tick),
        }
    }

    /// 领一份发送端。**每个持有者一份**：`futures` 的保底槽位是按 sender 记的，
    /// 共用一份就等于共用一个槽位（`io_task::DoneDebt` 的文档细说了为什么这件
    /// 事关乎「泵会不会为一个已经没了的调用永远等下去」）。
    pub(crate) fn sender(&self) -> IoSender {
        self.tx.clone()
    }

    /// 把一个 IO future 交给泵驱动。`&self` 就够——`FuturesUnordered::push` 本身
    /// 只要求共享引用，于是 `dispatch::run_effect` 不必为了起飞而拿到独占借用。
    pub(crate) fn start(&self, task: impl Future<Output = ()> + 'static) {
        self.tasks.push(task.boxed_local());
    }

    /// 等一条 IO 消息，最多等 `timeout`。`None` = 这段时间里没有消息（心跳把泵
    /// 叫醒了），泵据此回到循环顶部扫截止线、看取消标志。
    ///
    /// **每次 poll 都先推一遍在飞的 IO future**：它们的进展只可能发生在这里
    /// （单线程上没有别人替它们跑），漏掉这一步 = 029 的并行退化成串行。
    pub(crate) async fn receive(&mut self, timeout: Duration) -> Option<IoMsg> {
        let deadline = Instant::now() + timeout;
        poll_fn(move |cx| {
            // 先登记再检查：中间来的那一次心跳不会被漏掉。
            self.heartbeat.register(cx.waker());
            self.drive(cx);
            if let Poll::Ready(message) = self.rx.poll_next_unpin(cx) {
                // `None`（所有发送端都没了）结构上不可达——泵自己握着一份。真
                // 出现也当成一次空转，下一圈截止线扫描会兜住。
                return Poll::Ready(message);
            }
            if Instant::now() >= deadline {
                return Poll::Ready(None);
            }
            Poll::Pending
        })
        .await
    }

    /// 推一遍所有在飞的 IO future，把跑完的收掉。
    ///
    /// `FuturesUnordered::poll_next` 一次调用会 poll 掉**所有**被唤醒的 future
    /// （这就是并发本身），只在其中某一个真的跑完时才返回 `Ready(Some(_))`；
    /// 空表返回 `Ready(None)`。两种 `Ready` 都不代表「没别的活了」，所以这里
    /// 循环到 `Pending`/`None` 为止。
    fn drive(&mut self, cx: &mut Context<'_>) {
        while let Poll::Ready(Some(())) = self.tasks.poll_next_unpin(cx) {}
    }
}

#[cfg(test)]
impl IoBus {
    /// 只推 IO future、不收消息。**测试专用**：真实的泵里这一步永远是
    /// `receive` 的一部分，拆出来是为了能在「发送端已经把增量写进 channel、泵
    /// 还没取走」这个**中间态**上做断言——117 引入的幽灵增量窗口正是它。
    pub(crate) fn drive_tasks_once(&mut self) {
        crate::block_on(poll_fn(|cx| {
            self.drive(cx);
            Poll::Ready(())
        }));
    }
}
