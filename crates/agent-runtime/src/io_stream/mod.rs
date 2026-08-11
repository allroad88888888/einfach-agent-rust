//! 「一次 provider 请求 → 一串可以 `await` 的行」——117 之后本 crate 里**唯一**
//! 还需要按平台分身的 IO 环节。**本模块只定契约与平台二选一**，两份实现各在一个
//! 文件里（[`native`]/`web`）。
//!
//! # 为什么接缝切在「行」这一层
//!
//! 117 把 `CallProvider` 的载体从 `std::thread` 换成了并发 future
//! （[`crate::io_task`]），可是「怎么把字节读成行」两个平台没法共用：
//!
//! - native 的 HTTP 客户端是 **ureq，物理上是阻塞的**，`post_stream` 里的 `read`
//!   没有任何非阻塞形态；
//! - 浏览器的 `fetch` 只有异步形态，`post_stream_async` 是个 `Future`，而
//!   `wasm32-unknown-unknown` 上**没有任何办法同步阻塞等一个 Promise**
//!   （`agent_transport::fetch_client` 模块文档记着这条实测结论）。
//!
//! 切在「行」这一层，接缝两侧就都只剩一件事：接缝之下是「字节怎么来」，接缝之上
//! （[`crate::io_task`]）是累积器、`(agent, attempt)` 信封、欠债—还债。后者**两个
//! 目标共用同一份代码，一个 cfg 都没有**——这是这条接缝位置正确的判据。
//!
//! # 契约（两份实现必须逐条对齐）
//!
//! `open(binding, request, cancel) -> mpsc::Receiver<StreamItem>`：
//!
//! 1. **同步返回接收端，请求当场起飞**——`provider_call::start`「只起飞不落地」
//!    的语义不变；
//! 2. 送回来的 [`StreamItem`] 顺序固定：`Prepared` 恰好一条（或者以
//!    `PreparationFailed` 收场），之后任意多条 `Line`，最后恰好一条 `Done`；
//! 3. 接收端被丢掉（泵收工 / 这次调用被放弃）之后，生产侧要自己收摊，**不需要
//!    也不允许**被 join 或物理中断。
//!
//! 少了 `Done` 不是错误状态，是「欠债没还」：[`crate::io_task`] 的 `DoneDebt`
//! 会在 future 被丢掉时替它还上一条终态消息（117 验收第三条）。

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;

#[cfg(not(target_arch = "wasm32"))]
pub(crate) use native::open;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::open;

use std::sync::Arc;

use agent_transport::{StreamOutcome, TransportError};

use crate::image_preparation_failure::ImagePreparationFailure;

/// 行源送回来的东西。顺序见模块文档「契约」第 2 条。
///
/// **少了 `Done` 不是错误状态，是「欠债没还」**：[`crate::io_task`] 的
/// `DoneDebt` 会在 future 被丢掉时替它还上一条终态消息，泵因此不会为一个已经
/// 没人管的调用干等（117 验收第三条）。
pub(crate) enum StreamItem {
    /// 请求准备完成（图片上传/重编码那一步），带回**本次请求专属**的图片引用。
    /// 落地时要拿它做终态脱敏，所以必须从这条路带回来，不能落地时再算一遍。
    Prepared(Vec<Arc<str>>),
    /// 准备阶段失败（含被取消）。
    PreparationFailed(ImagePreparationFailure),
    /// 流式响应体的一行，已经按 `\r\n`/`\n` 去过尾。
    Line(String),
    /// 这次请求读到头的方式。
    Done(Result<StreamOutcome, TransportError>),
}
