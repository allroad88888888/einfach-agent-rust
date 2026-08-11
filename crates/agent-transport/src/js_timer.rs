//! 浏览器/Node 通用的定时器桥接：`setTimeout` 包成一个 `Future`，以及一个
//! 不依赖 `futures`/`tokio` 系列 crate 的最小两路 race。两个都很小，但被
//! `fetch_client.rs`（退避等待）与 `web_stream_source.rs`（取消观察者）
//! 各用一次，抽出来避免重复。

use std::future::Future;
use std::task::Poll;

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// 等待 `ms` 毫秒。不经 `web_sys::window()`——同一份代码要能在主线程、
/// Worker、以及本 crate 用 Node 跑的 wasm 测试里都拿到 `setTimeout`，三者
/// 共同点只有「全局作用域上有这个函数」，所以直接反射取（跟
/// `fetch_request.rs` 里拿全局 `fetch` 是同一个理由）。
pub(crate) async fn delay_ms(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let global = js_sys::global();
        if let Ok(set_timeout) = js_sys::Reflect::get(&global, &JsValue::from_str("setTimeout"))
            && let Ok(set_timeout) = set_timeout.dyn_into::<js_sys::Function>()
        {
            let _ = set_timeout.call2(&global, &resolve, &JsValue::from_f64(ms as f64));
        }
    });
    let _ = JsFuture::from(promise).await;
}

/// `race` 的结果：哪一路先 ready。
pub(crate) enum Either<L, R> {
    Left(L),
    Right(R),
}

/// 轮流 poll 两个 future，谁先 `Ready` 就返回谁，另一个直接丢弃——不需要
/// `futures`/`tokio`，`std::future::poll_fn`（1.64 起稳定）够用。
///
/// 用途：[`web_stream_source::WebStreamSource::next_chunk`] 用它把
/// `reader.read()`（可能永远不 resolve——服务端还在但没数据）跟一个「每隔
/// `poll_interval` 查一次 `cancel`」的 future 赛跑，这样即使卡在一次真实的
/// 网络等待里，取消标志也能在有限时间内被观察到并触发
/// `AbortController::abort()`——`drive_stream` 自己那层「处理下一块字节前
/// 查一次 `cancel`」只能覆盖两次成功读取之间的间隙，管不到「卡在一次读
/// 中间」这种情况，这正是 native 侧靠读线程 + 主流程 `recv_timeout` 解耦
/// 出来的能力，这里用两个协作式 future 赛跑达到同样效果。
pub(crate) async fn race<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let mut a = Box::pin(a);
    let mut b = Box::pin(b);
    std::future::poll_fn(move |cx| {
        if let Poll::Ready(v) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(v));
        }
        if let Poll::Ready(v) = b.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(v));
        }
        Poll::Pending
    })
    .await
}
