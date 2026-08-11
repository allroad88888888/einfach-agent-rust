//! 行源的 **native 实现**：一条只把字节读成行的工作线程。契约见 [`super`] 的
//! 模块文档，这里只说这一份实现自己的取舍。
//!
//! # 为什么必须有一条工作线程
//!
//! native 的 HTTP 客户端是 **ureq，物理上是阻塞的**：`post_stream` 里的 `read`
//! 没有任何非阻塞形态，调用它的那一刻线程就走不了。谁来扛这份阻塞，只有两种
//! 可能：
//!
//! 1. 在泵的线程上扛 —— 那就是把 029 的并行当场掐死（一个调用阻塞住整个事件
//!    循环，其余在飞的 future 一个都 poll 不到），而且**不报错，只变慢**；
//! 2. 在一个工作线程上扛，行经 channel 回到泵所在的那一个线程。
//!
//! 本文件是 2。**这不是把 `io_thread.rs` 换个名字**：旧的 IO 线程上跑的是「发
//! 请求 + 累积 + 组终态消息 + 认领相关的一切」，新的工作线程上只剩「把字节读成
//! 行」，累积器、`(agent, attempt)` 信封、会合背压、欠债—还债全部搬回了泵所在
//! 的单线程上（[`crate::io_task`]）。
//!
//! # 背压：这一段也是会合的
//!
//! 行 channel 的容量同样是 0（`futures` 的语义是「每个 sender 保底一个槽位」，
//! 见 [`crate::io_bus`] 的说明）——工作线程发一行就得等泵那边的 future 取走，
//! 不会在内存里攒行。工作线程用 [`crate::block_on`] 把这次发送**阻塞**等到底：
//! 它阻塞的是自己这条工作线程，不是泵。
//!
//! # 放弃一个在飞调用之后
//!
//! 泵不 join、也不物理中断这条工作线程（跟 117 之前逐字同一个取舍）。接收端一
//! 旦随 future 一起被丢掉，工作线程的下一次发送立刻拿到 `Err`（futures 的
//! channel 在接收端 drop 时会把停在发送上的 sender 全部唤醒——已实测），于是
//! 它自己收摊；已经进入 ureq 阻塞读的那一次读允许物理跑完。

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

use futures_channel::mpsc;
use futures_util::SinkExt;

use crate::execution_binding::ExecutionBinding;
use crate::image_materialization::ProviderRequest;

use super::StreamItem;

/// 起一个行源。**同步返回接收端，请求当场起飞**——跟 117 之前
/// `io_thread::spawn` 立刻发请求的时机逐字一致，`provider_call::start`
/// 「只起飞不落地」的语义不变。
pub(crate) fn open(
    binding: ExecutionBinding,
    request: ProviderRequest,
    cancel: Arc<AtomicBool>,
) -> mpsc::Receiver<StreamItem> {
    // 容量 0：见模块文档「背压」。
    let (tx, rx) = mpsc::channel::<StreamItem>(0);
    thread::spawn(move || blocking_source(tx, binding, request, cancel));
    rx
}

/// 工作线程本体：准备请求 → 发出去 → 逐行往回送。**所有阻塞都发生在这里。**
///
/// 任何一次发送失败都说明接收端没了（泵收工，或者这次调用连同它的 future 一起
/// 被丢掉），直接收摊——不再读下一行，也不再尝试送终态。
fn blocking_source(
    mut tx: mpsc::Sender<StreamItem>,
    binding: ExecutionBinding,
    request: ProviderRequest,
    cancel: Arc<AtomicBool>,
) {
    let prepared = match request.prepare(&binding, &cancel) {
        Ok(prepared) => prepared,
        Err(failure) => {
            let _ = crate::block_on(tx.send(StreamItem::PreparationFailed(failure)));
            return;
        }
    };
    let references = prepared.private_references().to_vec();
    if crate::block_on(tx.send(StreamItem::Prepared(references))).is_err() {
        return;
    }
    let result = binding.client.post_stream(
        &binding.endpoint,
        &binding.api_key,
        prepared.body(),
        &cancel,
        |line| match crate::block_on(tx.send(StreamItem::Line(line.to_owned()))) {
            Ok(()) => ControlFlow::Continue(()),
            // 接收端没了：泵已经收工（或者已经放弃这次调用），没有理由继续读。
            Err(_) => ControlFlow::Break(()),
        },
    );
    let _ = crate::block_on(tx.send(StreamItem::Done(result)));
}
