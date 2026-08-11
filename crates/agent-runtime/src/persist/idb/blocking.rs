//! [`run_to_completion`]：把一个 future 跑到底、阻塞调用线程。
//!
//! `agent-runtime` 顶层已经有一份同形状的 `crate::block_on`（116/117 的临时桥），
//! 这里不复用它、自己再写一份约 25 行的原因是**隔离**：117 正在同一个分支上重构那
//! 份桥（它自己的模块文档写着「117 拆」），本模块的活动范围锁定在 `persist/` 之
//! 下，不该去依赖一个正在被另一个 issue 改动的文件——万一它被搬走或改形状，这里会
//! 无端被牵连。两份实现都是 [`std::task::Wake`] 文档给的教科书写法（`thread::park`/
//! `unpark` 配一个只会 `unpark` 的 `Waker`），没有自由发挥的空间，犯不上为了不重复
//! 这 25 行去抢那个文件的编辑权。
//!
//! **只给 native 用**：[`super::worker`]（专用 IO 线程）和
//! [`super::store::IdbStore::load`] 用它把 [`super::kv::KvStore`] 的异步调用变成
//! `SessionStore` 端口要求的同步返回值——两处都发生在真的 OS 线程上，`thread::park`
//! 在那里是合法的阻塞。真正的浏览器实现（[`super::web_kv`]，wasm 单线程、没有 OS
//! 线程）用不上也用不了这个文件：`wasm32-unknown-unknown` 上没有别的线程能
//! `unpark` 调用线程——那是宿主要解决的另一个问题（114c 的 wasm 接线），不是这个
//! 文件的职责，所以整个模块只在非 wasm32 编译。

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

/// 把一个 future 跑到底，返回它的输出。**阻塞调用线程**——调用方（`worker.rs` 的
/// 消息循环、`store.rs` 的 `load`）都只在这个线程上跑这一个 future，没有别的任务
/// 要抢这段时间。
pub(super) fn run_to_completion<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    let waker = std::task::Waker::from(Arc::new(ThreadWaker(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run_to_completion;

    #[test]
    fn runs_a_ready_future_to_completion() {
        assert_eq!(run_to_completion(async { 1 + 1 }), 2);
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

        assert_eq!(run_to_completion(YieldOnce(false)), "done");
    }
}
