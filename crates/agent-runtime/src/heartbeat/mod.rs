//! 泵的心跳：每隔 `POLL_INTERVAL` 把它叫醒一次。**本模块只定契约与平台二选一**，
//! 两份实现各在一个文件里（[`native`]/`web`）。
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
//! # 契约（两个目标逐字相同，泵那一侧一行不用改）
//!
//! - `Heartbeat::start(interval)`：起一条心跳，**随创建者一起活、一起死**。
//! - `Heartbeat::register(&self, waker)`：登记「下一次心跳叫醒我」，每次 poll
//!   都调一次（执行器换了 waker 也不会漏掉唤醒）。
//! - `Drop`：停掉心跳，不留一个永远在跑的东西。
//!
//! # 两份实现，以及为什么它们不是同一件事的两种写法
//!
//! | 目标 | 载体 | 为什么只能是它 |
//! |---|---|---|
//! | native（[`native`]） | 一条只睡觉、只叫人的 `std::thread` | 本仓不引 tokio（115 决策 2），`futures-util` 也不带 timer，native 的 async 世界里没有现成定时器可用 |
//! | wasm32（`web`） | `setInterval` / `clearInterval` | `wasm32-unknown-unknown` 上 `thread::spawn`/`thread::sleep` 编得过、一调就 trap（无 atomics/SharedArrayBuffer）；浏览器的定时器本来就是事件循环原生设施 |
//!
//! 两边都只做同一件事：把「间隔到了」翻译成一次 `Waker::wake`。**心跳不碰
//! socket、不碰状态、不发消息**——这正是把它单独拎成一个东西的理由：泵那一侧
//! （`crate::io_bus`）在两个目标上看到的是同一个 `Heartbeat`，没有任何 cfg。

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::Heartbeat;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::Heartbeat;
