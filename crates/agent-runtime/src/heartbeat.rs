//! 泵的心跳：每隔 `POLL_INTERVAL` 把它叫醒一次。
//!
//! # 为什么泵必须能「什么都没发生也醒过来」
//!
//! 117 之前这件事藏在 `rx.recv_timeout(POLL_INTERVAL)` 的那个 `timeout` 参数
//! 里，没人把它当成一件独立的事。换成 async channel 之后它必须被显式做出来，
//! 否则**两条既有能力会静默失效**（两条都不报错，只表现为「卡住」）：
//!
//! 1. **截止线**（`crate::deadline::sweep`）。服务端写完响应头就再也不吭声时，
//!    channel 上一个字节都不会来——没有心跳，泵会永远停在等待里，provider 超时
//!    永远不到点。`tests/it/timeout.rs` 盯着这条。
//! 2. **Ctrl-C**（`RunnerCtx::cancel_flag`）。它是另一条线程翻的一枚
//!    `AtomicBool`，翻它不会唤醒任何 future；泵要靠回到循环顶部去看它，才能把
//!    取消传播进每个在飞调用的 call-local 标志。`tests/it/cancel.rs` 盯着这条。
//!
//! # 为什么是一条线程，以及它凭什么不算「IO 载体又长回来了」
//!
//! native 的 async 世界里没有现成的定时器可用：本仓不引 tokio（115 决策 2），
//! `futures-util` 也不带 timer。所以每个 [`crate::io_bus::IoBus`] 起一条**只睡
//! 觉、只叫人**的线程：它不碰 socket、不碰状态、不发消息，只是把「20ms 到了」
//! 这件事翻译成一次 `Waker::wake`。语义与 117 之前的 `recv_timeout(20ms)` 逐字
//! 相同（那一版也是每 20ms 醒一次），成本是每轮一条短命线程。
//!
//! wasm 上这个文件换成 `setInterval` + `clearInterval`（drop 时清），泵那一侧
//! 一行不用改——这正是把心跳单独拎成一个东西的理由。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::thread;
use std::time::Duration;

/// 一条心跳。**随创建者一起活、一起死**：`Drop` 置停止位，后台线程在下一个
/// 间隔醒来时看到它就退出。
pub(crate) struct Heartbeat {
    shared: Arc<Shared>,
}

struct Shared {
    /// 当前该叫醒谁。泵每次 poll 都会把自己的 waker 装进来（`will_wake` 判重，
    /// 同一个 waker 不重复克隆）。
    waker: Mutex<Option<Waker>>,
    stopped: AtomicBool,
}

impl Heartbeat {
    pub(crate) fn start(interval: Duration) -> Self {
        let shared = Arc::new(Shared {
            waker: Mutex::new(None),
            stopped: AtomicBool::new(false),
        });
        let background = Arc::clone(&shared);
        thread::spawn(move || {
            while !background.stopped.load(Ordering::Acquire) {
                thread::sleep(interval);
                // 先把 waker 取出来再叫，不要拿着锁调外部代码。
                let waker = background.waker.lock().unwrap().clone();
                if let Some(waker) = waker {
                    waker.wake();
                }
            }
        });
        Heartbeat { shared }
    }

    /// 登记「下一次心跳叫醒我」。每次 poll 都调一次：执行器换了 waker 也不会
    /// 漏掉唤醒。
    pub(crate) fn register(&self, waker: &Waker) {
        let mut slot = self.shared.waker.lock().unwrap();
        if !slot.as_ref().is_some_and(|known| known.will_wake(waker)) {
            *slot = Some(waker.clone());
        }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.shared.stopped.store(true, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::Heartbeat;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Wake, Waker};
    use std::time::Duration;

    struct Counter(AtomicUsize);
    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 登记之后真的会被反复叫醒——泵的截止线扫描与取消标志轮询全靠这个。
    #[test]
    fn keeps_waking_the_registered_waker() {
        let counter = Arc::new(Counter(AtomicUsize::new(0)));
        let heartbeat = Heartbeat::start(Duration::from_millis(5));
        heartbeat.register(&Waker::from(Arc::clone(&counter)));
        std::thread::sleep(Duration::from_millis(120));
        let ticks = counter.0.load(Ordering::Relaxed);
        assert!(ticks >= 3, "120ms 里 5ms 的心跳该叫醒不止一次：{ticks}");

        // 丢掉之后最多再叫一次就该彻底停下。
        drop(heartbeat);
        std::thread::sleep(Duration::from_millis(60));
        let after_drop = counter.0.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(
            counter.0.load(Ordering::Relaxed),
            after_drop,
            "心跳该随 Heartbeat 一起死掉，不能留一条永远在跑的线程"
        );
    }
}
