//! [`SubscriberGuard`]：一条 SSE 连接存活期间的 RAII 句柄（issue 031「断开取消：
//! 引用计数订阅者，归零起 5s 宽限计时（可配）；宽限期内重连取消计时」）。
//!
//! 创建时给 [`SseHub`] 的订阅计数 `+1`，并且顺手打断上一次可能还在倒计时的
//! 宽限计时器——这就是「重连取消计时」的全部实现：不需要另开一条「是不是在
//! 重连」的判断，任何新连接到来都天然满足「有人回来了，不该再倒数」。
//! `Drop` 时 `-1`，归零才真正起一个新的宽限计时器；到点一看计数还是零，才调
//! [`CancelHandle::cancel`](crate::handle::CancelHandle::cancel)——不白烧 token
//! （ARCHITECTURE.md §传输「取消传播」）。059 之后 hub 存的是
//! `CancelHandle` 而不是整个 `SessionHandle`（`super` 模块文档），取消这条路
//! 一步没变：`CancelHandle::cancel` 就是 `SessionHandle::send(Command::Cancel)`
//! 内部走的同一条实现。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::SseHub;

pub(in crate::http) struct SubscriberGuard {
    hub: Arc<SseHub>,
}

impl SubscriberGuard {
    pub(in crate::http) fn attach(hub: Arc<SseHub>) -> Self {
        hub.subscribers.fetch_add(1, Ordering::SeqCst);
        if let Some(task) = hub.grace_task.lock().unwrap().take() {
            task.abort();
        }
        SubscriberGuard { hub }
    }
}

impl Drop for SubscriberGuard {
    fn drop(&mut self) {
        let previous = self.hub.subscribers.fetch_sub(1, Ordering::SeqCst);
        if previous != 1 {
            return; // 走到 0 之前还有别的订阅者在，不该起宽限计时。
        }
        let hub = Arc::clone(&self.hub);
        let task = tokio::spawn(async move {
            tokio::time::sleep(hub.grace).await;
            // 宽限期内可能又连回来了（`Self::attach` 已经 abort 过这个任务的话
            // 根本不会跑到这里；这里读到的是真的一直没人回来的情况）。
            if hub.subscribers.load(Ordering::SeqCst) == 0 {
                let _ = hub.canceller.cancel();
            }
        });
        *self.hub.grace_task.lock().unwrap() = Some(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::SessionId;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use std::time::Duration;
    use tokio::sync::broadcast;

    /// 一个不背后跑真 actor 的 hub——`SessionHandle::subscribe` 只是订阅一个
    /// `broadcast::Sender`，不需要真的有人往里发。
    ///
    /// 059 之后这个假 handle 在 `SseHub::spawn` 返回时就被 drop 了（hub 只留
    /// `CancelHandle`），于是那条 drain 任务当场收到 `None`、把
    /// `SessionId("guard-test")` 从这张一次性的表里摘掉然后退出——对下面三条
    /// 断言无害：宽限计时器读写的是测试自己手上这份 `Arc<SseHub>`（订阅计数、
    /// `grace_task`、`canceller`），跟 drain 任务和那张表没有关系。
    fn fake_hub(grace: Duration) -> Arc<SseHub> {
        let (tx, _rx) = mpsc::channel::<crate::Command>();
        let (events, _keep_alive) = broadcast::channel(4);
        let tree = Arc::new(Mutex::new(agent_core::AgentTree { nodes: Vec::new() }));
        let canceller = crate::handle::CancelHandle::new(tx, Arc::new(AtomicBool::new(false)));
        let pending_tools = Arc::new(Mutex::new(Vec::new()));
        let handle = crate::SessionHandle {
            canceller,
            events,
            tree,
            pending_tools,
        };
        let hubs = Arc::new(Mutex::new(HashMap::new()));
        SseHub::spawn(handle, 8, grace, SessionId::from("guard-test"), hubs)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn last_subscriber_leaving_cancels_after_the_grace_period() {
        let hub = fake_hub(Duration::from_millis(30));
        let guard = SubscriberGuard::attach(Arc::clone(&hub));
        drop(guard);
        assert!(!hub.canceller.is_cancelled(), "宽限期还没过，不该提前取消");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            hub.canceller.is_cancelled(),
            "宽限期过了、始终没人回来，该取消了"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reconnecting_within_the_grace_period_avoids_the_cancel() {
        let hub = fake_hub(Duration::from_millis(60));
        let first = SubscriberGuard::attach(Arc::clone(&hub));
        drop(first);
        tokio::time::sleep(Duration::from_millis(15)).await;
        let second = SubscriberGuard::attach(Arc::clone(&hub)); // 该 abort 掉上面那次倒计时
        tokio::time::sleep(Duration::from_millis(120)).await; // 远超原本的宽限期
        assert!(!hub.canceller.is_cancelled(), "宽限期内回来了，不该被取消");
        drop(second);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_second_concurrent_subscriber_prevents_the_countdown_from_even_starting() {
        let hub = fake_hub(Duration::from_millis(20));
        let first = SubscriberGuard::attach(Arc::clone(&hub));
        let second = SubscriberGuard::attach(Arc::clone(&hub));
        drop(first); // 还剩一个订阅者，不该起计时器
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            !hub.canceller.is_cancelled(),
            "断开的只是两个订阅者中的一个"
        );
        drop(second);
    }
}
