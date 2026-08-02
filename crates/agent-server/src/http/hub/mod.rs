//! [`SseHub`]：一个 session 的 SSE 面——环形缓冲（[`ring`]）+ 断开取消的引用计数
//! 与宽限计时（[`guard`]）+ 把两者接到一起的 drain / 转发任务，都在这个文件。
//!
//! # 一个 hub、一条 drain 任务、每条连接一条转发任务
//!
//! [`SseHub::spawn`] 起**一条**后台任务：订阅这个 session 的 [`SessionHandle`]
//! （034 起是 `broadcast<Frame>`——`Frame` 是 agent 归属信封，`crate::event::
//! frame` 模块文档），每收到一条就塞进 [`ring::RingState`] 分配帧 id（结果是
//! [`ring::BufferedFrame`]，**不要跟 `Frame` 混淆**：前者是这个模块自己的
//! 记账形状，后者是对外的协议信封），再转发到 hub 自己的
//! `live: broadcast::Sender<BufferedFrame>`。这条任务跟 session 同生共死——
//! `SessionHandle::subscribe` 返回 `None` 就说明 actor 线程没了，drain 任务把
//! 自己从 [`crate::http::state::AppState`] 的 hub 表里摘掉后退出（自清理，
//! 不需要谁来显式关这个 hub）。
//!
//! 每一条 `GET /sessions/:id/events` 连接各自 [`SseHub::spawn_forwarder`] 一条
//! **转发任务**：算出该怎么接上（[`ring::Replay`]）、把结果灌进这条连接专属的
//! `mpsc` 通道，再无缝续上 `live` 的直播。转发任务在 `tx.send` 失败时会自然
//! 终结（客户端断开、`mpsc::Receiver` 被 drop 之后），但那**不是**断开取消
//! 机制的触发点——见下一节。
//!
//! # `SubscriberGuard` 为什么不能活在转发任务里（031 独测踩过的真事故）
//!
//! 最初的写法是转发任务一开始 `SubscriberGuard::attach`、任务本体退出时
//! （因为某次 `tx.send` 失败）顺带 drop 掉它。**这在假上游一直不回数据的场景
//! 下会漏检断开**：转发任务补发完 backlog 之后一直卡在 `live_rx.recv().await`
//! 上等下一条事件——如果客户端在这之后断开、而这条 session 在断开之后**再没有
//! 广播过任何新事件**（正是「上游挂住」测试要制造的情况），转发任务压根不会
//! 再调用一次 `tx.send`，也就永远发现不了 `mpsc::Receiver`已经没了、`_guard`
//! 永远不会 drop、宽限计时器永远不会启动。独测里这表现为间歇性失败——运气好时
//! 断开前后恰好有一条别的事件路过，顺便把这次失败的 `send` 撞出来；运气不好
//! （比如这次输入之后再没有任何事件，直到 provider 真的超时）就一直卡着。
//!
//! 正确的触发点是 axum/hyper 自己：客户端断开连接，hyper 会丢弃它正在驱动的
//! 响应体（`Stream`），这个事实不依赖这个 `Stream` 有没有产出过任何东西。于是
//! `SubscriberGuard` 必须活在**这个 `Stream` 对象本身**里，而不是活在一个跟这个
//! `Stream` 只通过 `mpsc` 弱关联、独立生死的后台任务里——[`SseHub::spawn_forwarder`]
//! 现在把 `attach` 挪到转发任务之外、同步完成，guard 交还给调用方
//! （`crate::http::routes::sse`）在真正会被 axum drop 的那个 `Stream`（`.map`
//! 闭包）里持有它。
//!
//! # 补发和直播的接缝为什么不会漏一帧
//!
//! [`SseHub::spawn_forwarder`] 在**同一次持锁**里做两件事：读 `ring` 算 backlog、
//! 订阅 `live`。drain 任务往 `ring` 追加新帧和往 `live` 广播是两个动作，但追加
//! 那个动作需要拿同一把 `ring` 锁——于是「转发任务已经拿到 backlog 快照，但还没
//! 订阅上 live」这个窗口里，drain 任务不可能已经把新帧广播出去（它连追加都还没
//! 排上号，卡在同一把锁上）。等转发任务订阅完 `live` 释放锁，drain 才能继续，
//! 而这时转发任务已经不会错过它了。

mod guard;
mod ring;

use std::collections::HashMap;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use agent_core::AgentId;

use crate::SessionHandle;
use crate::event::{Frame, SessionEvent};
use crate::registry::SessionId;

pub(super) use guard::SubscriberGuard;
pub(super) use ring::BufferedFrame;
use ring::{Replay, RingState};

/// 每条转发任务发进 `mpsc` 通道的容量——只是给下游一点缓冲，不代表任何协议
/// 承诺，随便挑一个不至于让快速连续的几条事件互相卡背的数字。
const FORWARD_CHANNEL_CAPACITY: usize = 64;

pub(super) struct SseHub {
    handle: SessionHandle,
    ring: Mutex<RingState>,
    live: broadcast::Sender<BufferedFrame>,
    subscribers: AtomicUsize,
    grace: Duration,
    grace_task: Mutex<Option<JoinHandle<()>>>,
}

impl SseHub {
    /// 起一个 hub：drain 任务立刻开始订阅并缓冲，哪怕这一刻还没有任何 SSE
    /// 连接——不然「先 `POST /sessions/:id/input`，稍后才第一次
    /// `GET /events`」这种顺序会在 hub 存在之前的这段时间里丢事件。
    ///
    /// `hubs` 是 [`crate::http::state::AppState`] 持有的那张表的共享引用——drain
    /// 任务退出（session 死了）时把自己摘掉，这是这个 hub 生命周期唯一的清理
    /// 动作，不需要额外的「关闭」入口。
    pub(super) fn spawn(
        handle: SessionHandle,
        ring_capacity: usize,
        grace: Duration,
        id: SessionId,
        hubs: Arc<Mutex<HashMap<SessionId, Arc<SseHub>>>>,
    ) -> Arc<Self> {
        // 容量给足一点余量：只要不比环形缓冲本身小，`live` 上出现 `Lagged`
        // 就意味着某条转发任务自己太慢，跟环形缓冲的补发能力无关（见
        // `forward_live` 文档）。
        let (live_tx, _keep_channel_alive) = broadcast::channel(ring_capacity.max(16));
        let hub = Arc::new(SseHub {
            handle: handle.clone(),
            ring: Mutex::new(RingState::new(ring_capacity)),
            live: live_tx,
            subscribers: AtomicUsize::new(0),
            grace,
            grace_task: Mutex::new(None),
        });

        let drain_hub = Arc::clone(&hub);
        tokio::spawn(async move {
            let mut sub = handle.subscribe();
            // 034：`sub.recv()` 给的是 `Frame`（agent + event 信封），不再是裸
            // 的 `SessionEvent`——`ring.push` 直接收它，配一个 SSE 帧 id。
            while let Some(envelope) = sub.recv().await {
                let buffered = {
                    let mut ring = drain_hub.ring.lock().unwrap();
                    ring.push(envelope)
                };
                let _ = drain_hub.live.send(buffered); // 没有活跃订阅者时 Err，无害。
            }
            hubs.lock().unwrap().remove(&id);
        });

        hub
    }

    /// 给一条新的 SSE 连接起一条转发任务，返回它专属的接收端（路由层把这个
    /// `mpsc::Receiver` 包成 axum 的 SSE `Stream`）和一个 [`SubscriberGuard`]
    /// ——**调用方必须把这个 guard 的存活期绑定到那个真正会被 axum/hyper
    /// drop 的 `Stream` 对象上**（本文件模块文档「`SubscriberGuard` 为什么不能
    /// 活在转发任务里」），不能自己另外找个地方存着，也不能索性丢弃不用。
    #[must_use = "SubscriberGuard 必须绑定到会被 axum 在客户端断开时 drop 的 Stream 上，见 crate::http::hub 模块文档"]
    pub(super) fn spawn_forwarder(self: &Arc<Self>, last_event_id: Option<u64>) -> (mpsc::Receiver<BufferedFrame>, SubscriberGuard) {
        let (tx, rx) = mpsc::channel(FORWARD_CHANNEL_CAPACITY);

        // 见模块文档「补发和直播的接缝为什么不会漏一帧」：`replay` 和
        // `live.subscribe()` 必须在同一次持锁里做。
        let (replay, live_rx) = {
            let ring = self.ring.lock().unwrap();
            let live_rx = self.live.subscribe();
            (ring.replay(last_event_id), live_rx)
        };

        // 同步 attach——这一刻就是「这条连接算不算一个订阅者」的真实生效时间，
        // 不等转发任务哪天被调度到。
        let guard = SubscriberGuard::attach(Arc::clone(self));

        tokio::spawn(async move {
            if send_replay(&tx, replay).await {
                forward_live(live_rx, &tx).await;
            }
        });

        (rx, guard)
    }
}

/// 把 [`Replay`] 的结果灌进 `tx`。返回 `false` = 对端已经没了（客户端在补发
/// 阶段就断了），调用方不必再继续接直播。
///
/// `Gap` 分支（031 独测分歧 2 的裁决）：gap 帧只代表「被冲掉、补不回来」的
/// 那一段，不是「放弃这条连接接下来的全部补发」——发完 gap 帧之后，紧接着把
/// `tail`（缓冲区里仍然保留的那一段）原样重放，跟 `Backlog` 分支同一套逐帧
/// 发送逻辑，最后才续上直播。
async fn send_replay(tx: &mpsc::Sender<BufferedFrame>, replay: Replay) -> bool {
    match replay {
        Replay::Live => true,
        Replay::Backlog(frames) => send_frames(tx, frames).await,
        Replay::Gap { skipped, gap_frame_id, tail } => {
            // 034：gap 帧标 root——它是重连补发算出来的传输层事实，不属于树上
            // 任何一个具体 agent（`crate::event::frame` 模块文档同一条判据）。
            let envelope = Frame { agent: AgentId::root(), event: SessionEvent::Gap { skipped } };
            if tx.send(BufferedFrame { id: gap_frame_id, event: envelope }).await.is_err() {
                return false;
            }
            send_frames(tx, tail).await
        }
    }
}

async fn send_frames(tx: &mpsc::Sender<BufferedFrame>, frames: Vec<BufferedFrame>) -> bool {
    for frame in frames {
        if tx.send(frame).await.is_err() {
            return false;
        }
    }
    true
}

/// 补发完了，接上直播：把 `live` 广播的每一帧转发进这条连接专属的 `tx`。
async fn forward_live(mut live_rx: broadcast::Receiver<BufferedFrame>, tx: &mpsc::Sender<BufferedFrame>) {
    loop {
        match live_rx.recv().await {
            Ok(frame) => {
                if tx.send(frame).await.is_err() {
                    return; // 客户端断开：mpsc 接收端没了。
                }
            }
            // 这条转发任务自己太慢，把 `live` 的容量吃穿了——环形缓冲本身够大
            // （`spawn` 里的注释），这不是「补发能力不够」，是这一条连接积压。
            // 直接断开：真实的 `EventSource` 客户端会带着它自己见过的最后一个
            // `Last-Event-ID` 原生重连，接上 `ring` 精确补发/gap 的逻辑，比在
            // 这里硬凑一条不知道该标几号的合成帧更诚实。
            Err(broadcast::error::RecvError::Lagged(_)) => return,
            Err(broadcast::error::RecvError::Closed) => return, // hub 没了（session 死了）。
        }
    }
}
