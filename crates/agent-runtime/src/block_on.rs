//! 116（临时桥，117 拆）：手写的最小 `block_on`。
//!
//! 115 原文写的建议路径是 `futures_util::executor::block_on`——**实测这个路径
//! 不存在**：`executor`（`LocalPool`/`block_on` 那一整套）活在 `futures-executor`
//! 这个独立 crate 里，`futures` 全量门面才把它转发成 `futures::executor`；
//! `futures-util` 本身从来没有 `executor` 模块，0.3.33 的 feature 列表里也没有
//! 叫这个名字的 feature（`cargo build` 报的 `E0432`/`error: failed to select a
//! version` 两次都实测到了）。加 `futures-executor` 能解决，但会在「futures 最小
//! 子集」之外再添一个 crate——115 的验收原话是「只有 futures-core /
//! futures-util」，这里没有绕开它的余地。
//!
//! 115 同一段决策原文早就留好了这条口子：「`block_on`（约 30 行）自己写完全
//! 没问题，错了当场暴露；但会合 channel 自己写……」——risky 的是手写会合
//! channel（117 才会碰：`futures::channel::mpsc` 换 `sync_channel(0)` 那一步），
//! 不是这个。`block_on` 本身是 Rust 标准库 `std::task::Wake` 文档给的教科书
//! 实现（[`std::task::Wake`] 的 example 就是这个形状），没有自由发挥的空间，
//! 犯不上为它多背一个 crate。
//!
//! 两个约束决定了它必须长这样：
//! - **零 futures 依赖**：只用 `std`，`futures-core`/`futures-util` 仍然按 115
//!   的决定留在 `Cargo.toml` 里（`agent-runtime`/`agent-cli` 两处），但那是为
//!   117 真正接 `futures_util::channel::mpsc` 准备的，不是这个函数要用的。
//! - **单 future、无并发**：调用方（`agent-cli`、`agent-server` 的 session
//!   actor——它是裸 `std::thread`，不在 tokio 运行时里）都只在一个线程上跑一个
//!   顶层 future 到底，不需要任务队列、不需要 `Send`/`'static` 约束，`thread::
//!   park`/`unpark` 配 `std::task::Wake` 就够。

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake};
use std::thread::{self, Thread};

/// 把当前线程包成一个 `Waker`：被唤醒时 `unpark` 它。
struct ThreadWaker(Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// 把一个 future 跑到底，返回它的输出。**阻塞调用线程**——116 的所有调用方
/// 都只在这个线程上跑这一个 future，没有别的任务要抢这段时间。
pub fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = std::task::Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            // `runner::receive`（116 的另一半桥）目前从不真正返回 `Pending`——
            // 它的 poll 体全是阻塞代码，一次 `poll` 调用就能跑到 `Ready`。这个
            // 分支现在打不到，但 117 把 `receive` 换成真正的非阻塞 poll 之后，
            // 这里就是它开始生效的地方：`Wake::wake` 会在真正有数据可读时把
            // 这个线程 `unpark`，不是空转。
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::block_on;

    #[test]
    fn runs_a_ready_future_to_completion() {
        assert_eq!(block_on(async { 1 + 1 }), 2);
    }

    #[test]
    fn runs_a_future_that_yields_pending_at_least_once() {
        use std::task::Poll;

        struct YieldOnce(bool);
        impl std::future::Future for YieldOnce {
            type Output = &'static str;
            fn poll(
                mut self: std::pin::Pin<&mut Self>,
                cx: &mut std::task::Context<'_>,
            ) -> Poll<Self::Output> {
                if self.0 {
                    Poll::Ready("done")
                } else {
                    self.0 = true;
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            }
        }

        assert_eq!(block_on(YieldOnce(false)), "done");
    }
}
