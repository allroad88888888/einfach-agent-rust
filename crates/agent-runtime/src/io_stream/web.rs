//! 行源的 **wasm32 实现**：`fetch` + `ReadableStream`，一条线程都不起。契约见
//! [`super`] 的模块文档，这里只说这一份实现自己的取舍。
//!
//! # 载体：两个 `spawn_local` 任务，不是线程
//!
//! `wasm32-unknown-unknown` 上没有线程（`thread::spawn` 编得过、一调就 trap），
//! 但也**不需要**：`fetch` 的响应体本来就是异步的 `ReadableStream`，
//! `agent_transport::Client::post_stream_async` 已经把「发请求 + 分帧 + 退避重试」
//! 做成了一个 `Future`。所以 `open` 把它交给
//! `wasm_bindgen_futures::spawn_local`，由浏览器的事件循环驱动——跟泵自己是同一个
//! 事件循环，泵在 `IoBus::receive` 里让出线程的那段时间正好给它跑。
//!
//! # 为什么中间要多一条 unbounded channel（这不是多余的一层）
//!
//! `post_stream_async` 的 `on_line` 是**同步**回调（`FnMut(&str) -> ControlFlow`，
//! 与 native 的 `post_stream` 逐字同一份签名）。native 那份实现在回调里用
//! [`crate::block_on`] 把「把这一行交给泵」阻塞等到底——阻塞的是它自己那条工作
//! 线程。浏览器里没有这条路：**单线程模型下阻塞当前线程 = 连驱动 `fetch` 的事件
//! 循环一起停住**，死锁（`agent_transport::fetch_client` 模块文档实测记过这条）。
//!
//! 于是这里拆成两段，各自只做一件能做的事：
//!
//! ```text
//!   post_stream_async 的 on_line ──unbounded_send（同步、不阻塞）──▶ raw channel
//!                                                                        │
//!                                          forward 任务 ──tx.send().await（会合背压）──▶ 泵
//! ```
//!
//! **[`forward`] 是这条 channel 唯一的写入方**，所以 `Line` 与 `Done` 仍然严格
//! 按产生顺序到达泵（终态永远排在此前所有增量之后）——如果让 `Done` 绕过 raw
//! channel 直接发，它会插到还排队的行前面，泵会把一轮答到一半的回复当成已完成。
//!
//! # 代价：会合背压只剩后半段
//!
//! 泵与 [`forward`] 之间仍然是容量 0 的会合（跟 native 逐字相同）；`fetch` 读取
//! 与 [`forward`] 之间那一段没有背压，行会在 raw channel 里排队。这是**平台事实
//! 不是设计选择**：浏览器无论如何都在自己那层缓冲响应体，我们没有任何手段把
//! 背压传回 `ReadableStream`（同步回调里做不了 `await`）。排队量的上限是一次
//! 响应的行数，有界；而泵每次 `receive` 都会取走一条，实际很难堆起来。
//!
//! # 放弃一个在飞调用之后
//!
//! 泵丢掉接收端 → [`forward`] 的 `send` 立刻拿到 `Err` → 它返回、`raw_rx` 随之
//! 析构 → 生产侧下一次 `unbounded_send` 拿到 `Err` → `on_line` 返回
//! `ControlFlow::Break`，`drive_stream` 当场 `AbortController::abort()`。**取消
//! 因此比 native 更彻底**：native 那边已经进入 ureq 阻塞读的那一次读只能允许它
//! 物理跑完，这里连接是真的被断掉的。
//!
//! # 图片
//!
//! 114c 落地时这里还有一段「请求准备」（`ProviderRequest::prepare` 里的同步
//! `Client::upload_image` 在 wasm 上必定失败，落成一条 `PreparationFailed`）。
//! M12 的 s5 视觉重构把那条 legacy 图片管线整个换成了 uploads +
//! `srv:vision/inspect`，`prepare` 不复存在——请求体在 `provider_call::start`
//! 里就编好传下来了，这个文件因此**一次准备 IO 都不做**，正是 114c 真机验收跑
//! 的那条路。浏览器里的图片输入现在是宿主经 uploads 端点的事，不在这条线上。

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use futures_channel::mpsc;
use futures_util::{SinkExt, StreamExt};

use crate::execution_binding::ExecutionBinding;

use super::StreamItem;

/// 起一个行源。**同步返回接收端，请求当场起飞**——与 native 那份逐字同一句
/// 承诺，只是载体从 `thread::spawn` 换成了 `spawn_local`。
pub(crate) fn open(
    binding: ExecutionBinding,
    body: Vec<u8>,
    cancel: Arc<AtomicBool>,
) -> mpsc::Receiver<StreamItem> {
    // 容量 0：泵那一侧的会合背压，跟 native 逐字相同。
    let (tx, rx) = mpsc::channel::<StreamItem>(0);
    // 同步回调与「会合式发送」之间的缓冲，见模块文档。
    let (raw_tx, raw_rx) = mpsc::unbounded::<StreamItem>();
    wasm_bindgen_futures::spawn_local(produce(raw_tx, binding, body, cancel));
    wasm_bindgen_futures::spawn_local(forward(raw_rx, tx));
    rx
}

/// raw channel → 泵。**泵那条 channel 唯一的写入方**（顺序保证靠这一条，见模块
/// 文档）。一次发送失败就收摊：接收端没了 = 泵收工或这次调用被放弃。
async fn forward(mut raw_rx: mpsc::UnboundedReceiver<StreamItem>, mut tx: mpsc::Sender<StreamItem>) {
    while let Some(item) = raw_rx.next().await {
        if tx.send(item).await.is_err() {
            return;
        }
    }
}

/// 生产侧本体：把现成的请求体经 `fetch` 发出去 → 逐行往 raw channel 里塞。
///
/// 任何一次 `unbounded_send` 失败都说明 [`forward`] 已经收摊（连带说明泵不要这
/// 次调用了），当场停止——`on_line` 返回 `Break` 会让 `drive_stream` 立刻
/// `abort()` 掉底层连接。
async fn produce(
    raw_tx: mpsc::UnboundedSender<StreamItem>,
    binding: ExecutionBinding,
    body: Vec<u8>,
    cancel: Arc<AtomicBool>,
) {
    let line_tx = raw_tx.clone();
    let result = binding
        .client
        .post_stream_async(
            &binding.endpoint,
            &binding.api_key,
            &body,
            &cancel,
            |line| match line_tx.unbounded_send(StreamItem::Line(line.to_owned())) {
                Ok(()) => ControlFlow::Continue(()),
                // 接收端没了：泵已经收工（或者已经放弃这次调用），没有理由继续读。
                Err(_) => ControlFlow::Break(()),
            },
        )
        .await;
    let _ = raw_tx.unbounded_send(StreamItem::Done(result));
}
