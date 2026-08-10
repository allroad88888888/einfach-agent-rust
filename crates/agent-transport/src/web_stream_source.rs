//! [`stream_drive::ChunkSource`] 的生产实现：包一个真实的
//! `web_sys::ReadableStreamDefaultReader` + `AbortController`。
//!
//! 这是本 crate 里唯一直接碰 `web_sys` 流式读取的地方，刻意写得很薄——分帧
//! 逻辑全部在 `line_framer`/`stream_drive` 里，这里只做两件事：把 JS
//! Promise 翻译成 `Result<Option<Vec<u8>>, String>`；以及**给一次可能永远
//! 不 resolve 的 `read()` 配一个取消出口**（见 [`ChunkSource::next_chunk`]
//! 文档）——`drive_stream` 自己那层「处理下一块字节前查一次 cancel」只能
//! 覆盖两次成功读取之间的间隙，管不到「卡在一次读中间」，这正是 native 侧
//! 靠读线程 + 主流程 `recv_timeout` 解耦出来的能力，这里用
//! [`crate::js_timer::race`] 把 `reader.read()` 和一个轮询 `cancel` 的
//! 定时器赛跑，达到同样效果。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use js_sys::Uint8Array;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, ReadableStreamDefaultReader};

use crate::js_timer::{Either, delay_ms, race};
use crate::stream_drive::ChunkSource;

/// 包一个 `fetch` 响应体的 `ReadableStreamDefaultReader`。`abort()` 对应
/// native `read_loop::run` 取消时「立刻返回、不 join 读线程、直接丢弃连接」
/// 那句话——这里换成调 `AbortController::abort()`，浏览器负责把底层连接
/// 断掉，我们不等它。
pub(crate) struct WebStreamSource<'a> {
    reader: ReadableStreamDefaultReader,
    controller: AbortController,
    cancel: &'a AtomicBool,
    poll_interval: Duration,
}

impl<'a> WebStreamSource<'a> {
    pub(crate) fn new(
        reader: ReadableStreamDefaultReader,
        controller: AbortController,
        cancel: &'a AtomicBool,
        poll_interval: Duration,
    ) -> Self {
        WebStreamSource {
            reader,
            controller,
            cancel,
            poll_interval,
        }
    }
}

impl ChunkSource for WebStreamSource<'_> {
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
        let read = JsFuture::from(self.reader.read());
        let cancelled = wait_until_cancelled(self.cancel, self.poll_interval);
        match race(read, cancelled).await {
            Either::Left(read_result) => parse_read_result(read_result),
            Either::Right(()) => {
                // 卡在一次读中间时发现取消——`read()` 那个 Promise 还没
                // resolve，我们不等它，主动 abort 让浏览器把底层连接断掉。
                // 返回 `Ok(None)`：`drive_stream` 会再看一眼 `cancel`，把这
                // 种情况和真 EOF 分开（见 `ChunkSource::next_chunk` 文档）。
                self.controller.abort();
                Ok(None)
            }
        }
    }

    fn abort(&mut self) {
        self.controller.abort();
    }
}

fn parse_read_result(result: Result<JsValue, JsValue>) -> Result<Option<Vec<u8>>, String> {
    let result = result.map_err(|e| describe_js_error(&e))?;
    let done = js_sys::Reflect::get(&result, &JsValue::from_str("done"))
        .map(|v| v.is_truthy())
        .unwrap_or(false);
    if done {
        return Ok(None);
    }
    let value = js_sys::Reflect::get(&result, &JsValue::from_str("value"))
        .map_err(|e| describe_js_error(&e))?;
    let array: Uint8Array = value
        .dyn_into()
        .map_err(|_| "ReadableStream 吐出的 value 不是 Uint8Array".to_string())?;
    Ok(Some(array.to_vec()))
}

async fn wait_until_cancelled(cancel: &AtomicBool, poll_interval: Duration) {
    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        delay_ms(poll_interval.as_millis() as i32).await;
    }
}

/// `JsValue` 形式的错误没有统一的 `Display`；优先取 `Error.message`，退化到
/// `format!("{:?}")`。不含 API key——调用方（`fetch_client`）负责在拼进
/// `TransportError`/`UploadError` 之前做 `upload::redact`（如果那条路径带
/// key 的话；`post_stream` 这条本来就不把 key 放回错误里，跟 native
/// `TransportError` 的约束一致，见 `lib.rs` 顶部)。
pub(crate) fn describe_js_error(value: &JsValue) -> String {
    if let Some(err) = value.dyn_ref::<js_sys::Error>() {
        return err.message().into();
    }
    if let Some(s) = value.as_string() {
        return s;
    }
    format!("{value:?}")
}
