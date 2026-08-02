//! [`SessionHandle`]：外界持有的、指向一个 actor 线程的全部东西——一个
//! `mpsc::Sender<Command>`、一份可以直接旁路写的取消标志、一个 `broadcast`
//! 发送端（订阅入口）。**不含 `JoinHandle`**——`join` 是 [`crate::registry::
//! SessionRegistry`] 优雅关闭时才做的事，一个到处克隆的句柄不该持有它
//! （`JoinHandle` 也不是 `Clone`，硬塞会逼 `SessionHandle` 变得不能自由复制）。
//!
//! 每个字段都廉价可 `Clone`（`mpsc::Sender`/`Arc`/`broadcast::Sender` 皆然），
//! 于是 `SessionHandle` 本身整体 `Clone`——多个调用方（未来 031 的多个并发 HTTP
//! 请求）可以各自拿一份，互不干扰。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use tokio::sync::broadcast;

use agent_core::AgentId;

use crate::command::Command;
use crate::event::{Frame, SessionEvent};

/// `send` 失败的唯一理由：actor 线程已经不在了（`Shutdown` 处理完退出，或者
/// panic 之后 `mpsc::Receiver` 被丢弃）。不区分「正常关闭」和「崩溃」——
/// 那是 [`crate::registry::SessionRegistry::get`] 该回答的问题（它能读到死因），
/// 这里只回答「这条命令没送到」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionClosed;

impl std::fmt::Display for SessionClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "session 的 actor 线程已经不在了，命令没有送达")
    }
}

impl std::error::Error for SessionClosed {}

#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) tx: mpsc::Sender<Command>,
    pub(crate) cancel: Arc<AtomicBool>,
    /// 034：广播载荷是 [`Frame`]（agent 归属信封），不再是裸的 `SessionEvent`
    /// ——见 `crate::event::frame` 模块文档。
    pub(crate) events: broadcast::Sender<Frame>,
}

impl SessionHandle {
    /// 送一条命令。**`Command::Cancel` 在这里被特判**：不进 `mpsc` 队列，
    /// 直接把共享的取消原子标志翻成 `true`——旁路排队正是这条命令存在的意义
    /// （`crate::command` 模块文档）。其余变体照常入队，由 actor 线程按到达
    /// 顺序处理。
    ///
    /// 失败只有一种情况：actor 线程已经不在了（`mpsc::Receiver` 被丢弃）。
    /// `Cancel` 因为压根不碰队列，永远成功——即使 session 已经死了，翻一个
    /// 没人再读的标志也无害，不构成错误。
    pub fn send(&self, cmd: Command) -> Result<(), SessionClosed> {
        if matches!(cmd, Command::Cancel) {
            self.cancel.store(true, Ordering::Relaxed);
            return Ok(());
        }
        self.tx.send(cmd).map_err(|_| SessionClosed)
    }

    /// 便捷方法：等价于 `send(Command::Cancel)`，但类型上更直接地表达
    /// 「这是一次旁路操作，不是排队」。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 订阅这个 session 的事件流。新订阅者只看得见**订阅之后**广播的事件——
    /// `broadcast` 没有历史重放，`ARCHITECTURE.md` §传输 说的「actor 内保留一个
    /// 有界事件环形缓冲供补发」是 031 的 `Last-Event-ID` 重连语义，不是这一层
    /// 要做的事（030 的注意事项：这里还没有网络面）。
    pub fn subscribe(&self) -> Subscription {
        Subscription { inner: self.events.subscribe() }
    }
}

/// 对 `broadcast::Receiver<Frame>` 的一层薄包装：把 `Err(Lagged(n))`
/// 翻成一条显式的 [`SessionEvent::Lagged`] 事件，而不是要求每个调用方都重新
/// 学一遍 `tokio::sync::broadcast::error::RecvError` 的三态语义。`Closed`
/// （发送端——也就是整个 session——没了）翻成 `None`，跟一个走到头的迭代器
/// 同一种「没有更多了」的表达。
pub struct Subscription {
    inner: broadcast::Receiver<Frame>,
}

impl Subscription {
    /// 等下一条事件。`None` = 这个 session 的广播端彻底没了（actor 线程退出后
    /// 最后一个 `SessionHandle`/`broadcast::Sender` 也被丢弃）——正常关闭
    /// （`close()` 之后）和崩溃（`SessionDied` 广播完之后）都会走到这里，
    /// 崩溃那条路径在到达 `None` 之前已经先收到过一条 `SessionDied`。
    ///
    /// `Lagged` 合成的 [`Frame`] 标 [`AgentId::root`]——它是这条订阅连接本身
    /// 跟丢的事实，不属于树上任何一个具体 agent（`crate::event::frame` 模块
    /// 文档同一条判据）。
    pub async fn recv(&mut self) -> Option<Frame> {
        match self.inner.recv().await {
            Ok(frame) => Some(frame),
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                Some(Frame { agent: AgentId::root(), event: SessionEvent::Lagged { skipped } })
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle() -> (SessionHandle, mpsc::Receiver<Command>) {
        let (tx, rx) = mpsc::channel();
        let (events, _) = broadcast::channel(16);
        (SessionHandle { tx, cancel: Arc::new(AtomicBool::new(false)), events }, rx)
    }

    #[test]
    fn cancel_bypasses_the_queue_and_flips_the_flag_directly() {
        let (handle, rx) = handle();
        handle.send(Command::Cancel).unwrap();
        assert!(handle.cancel.load(Ordering::Relaxed), "旁路写标志该立刻生效");
        assert!(rx.try_recv().is_err(), "Cancel 不该出现在 mpsc 队列里");
    }

    #[test]
    fn other_commands_are_queued_in_order() {
        let (handle, rx) = handle();
        handle.send(Command::Input("a".to_string())).unwrap();
        handle.send(Command::Redo).unwrap();
        assert_eq!(rx.try_recv().unwrap(), Command::Input("a".to_string()));
        assert_eq!(rx.try_recv().unwrap(), Command::Redo);
    }

    #[test]
    fn send_fails_once_the_receiver_is_gone() {
        let (handle, rx) = handle();
        drop(rx);
        assert_eq!(handle.send(Command::Redo), Err(SessionClosed));
    }

    #[test]
    fn cancel_via_send_never_fails_even_after_the_actor_is_gone() {
        let (handle, rx) = handle();
        drop(rx);
        assert_eq!(handle.send(Command::Cancel), Ok(()), "旁路操作不该因为队列没人收而报错");
    }

    #[tokio::test]
    async fn lagged_receiver_gets_an_explicit_drop_event() {
        let (tx, _rx0) = broadcast::channel(2);
        let mut sub = Subscription { inner: tx.subscribe() };
        // 容量 2，连发 4 条 —— 订阅者肯定跟丢。
        for n in 0..4u64 {
            let _ = tx.send(Frame { agent: AgentId::root(), event: SessionEvent::Lagged { skipped: n } });
        }
        let first = sub.recv().await.unwrap();
        assert!(matches!(first.event, SessionEvent::Lagged { .. }), "该先看到掉帧事件：{first:?}");
    }
}
