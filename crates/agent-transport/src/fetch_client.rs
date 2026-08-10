//! wasm32 目标的流式 POST 客户端：`fetch` + `ReadableStream` +
//! `AbortController`，`Client::post_stream` 的 wasm 对应物。`lib.rs` 顶部
//! 「wasm 侧」一节说了为什么这条比 native 薄——分帧交给
//! [`crate::stream_drive::drive_stream`]，一次连接尝试的 `web_sys` 接线在
//! [`crate::fetch_request`]，这里只管「要不要重试、退避多久」这层策略。
//!
//! # `post_stream` 与 `post_stream_async`：一个绕不开的平台差异
//!
//! `Client::post_stream` 的签名（同步、`&self`、阻塞式返回
//! `Result<StreamOutcome, TransportError>`）是 native 那条定的，issue 113
//! 要求「上层看到的必须仍是同一个 `Client::post_stream`」。但 `fetch` 是
//! 天生异步的 Promise，`wasm32-unknown-unknown` 默认（无 `+atomics`）单线程
//! 模型下，**没有任何办法让一个同步函数真正阻塞等待一个 Promise 完成**——
//! 阻塞需要挂起当前线程，而当前线程如果被挂起，驱动那个 Promise resolve 的
//! JS 事件循环也转不动了，死锁。这不是本实现偷懒，是实测确认的平台限制：
//! `std::thread::spawn` 在这个目标上能编译，但一调用就以 `unreachable`
//! trap 收场（无 `+atomics`/`SharedArrayBuffer` 支持，见 113 实做记录）。
//!
//! 所以这里诚实地拆成两个方法：
//!
//! - [`Client::post_stream_async`]：真正的实现，`fetch` 全过程 + 分帧 +
//!   退避重试，`async fn`，是 114（wasm 宿主）接入时应该调的那个。
//! - [`Client::post_stream`]：签名与 native 逐字相同，**保留只是为了让
//!   `agent-providers`/`agent-runtime` 的源码在两个目标上都能编译通过**
//!   （它们今天的调用点是同步的，写在 `std::thread::spawn` 里）——但调用
//!   它本身不会真的发请求，立刻返回一个说明这件事的 `TransportError`。
//!   这是刻意的：宁可调用方在第一次拿到结果时就看到清楚的错误，也不要
//!   假装能阻塞、实际把整个页面卡死。谁把 `io_thread.rs` 接到 wasm 宿主
//!   上，谁就必须换成调 `post_stream_async`——那一步无法在不碰
//!   `agent-runtime` 的前提下于本 issue 内完成，如实记在 113 的实做记录里。
//!
//! `upload_image`/`upload_image_async` 是同一个道理，上传图片同样是异步
//! `fetch`。

use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use web_sys::AbortController;

use crate::backoff::Backoff;
use crate::fetch_request::{attempt_fetch, read_bounded_error_body, response_stream_source};
use crate::js_timer::delay_ms;
use crate::stream_drive::drive_stream;
use crate::upload::ImageUpload;
use crate::web_stream_source::describe_js_error;
use crate::{StreamOutcome, TransportError, UploadError};

/// 说明为什么 `post_stream`/`upload_image` 这两个同步入口不会真的发请求。
const WASM_SYNC_BLOCKING_UNSUPPORTED: &str = "wasm32 目标上 post_stream/upload_image 的同步签名无法真正阻塞等待 fetch（单线程模型下会死锁，浏览器不提供无 atomics 的阻塞原语）；改调 post_stream_async/upload_image_async，见 fetch_client.rs 模块文档";

pub struct Client {
    cancel_poll_interval: Duration,
    backoff: Backoff,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_config(
            Duration::from_secs(20),
            Duration::from_millis(100),
            Backoff::default(),
        )
    }

    /// 签名与 native `Client::with_config` 一致；`connect_timeout` 目前不
    /// 生效（存下来只为了 API 形状对齐）——`fetch` 没有内建的连接超时钩子，
    /// 要做需要另起一个定时器去竞速 `AbortController`，issue 113 没有要求
    /// 这一条，先不加，避免为了对齐一个没人验收的行为引入额外的 JS 交互
    /// 面。`cancel_poll_interval` 在这里只用于连接期退避等待的轮询节奏，
    /// 跟 native 同名字段的角色一致。
    pub fn with_config(
        _connect_timeout: Duration,
        cancel_poll_interval: Duration,
        backoff: Backoff,
    ) -> Self {
        Client {
            cancel_poll_interval,
            backoff,
        }
    }

    /// 与 native 签名逐字相同的入口，见模块文档「一个绕不开的平台差异」——
    /// 不发请求，立刻返回 [`WASM_SYNC_BLOCKING_UNSUPPORTED`]。
    pub fn post_stream(
        &self,
        _url: &str,
        _api_key: &str,
        _body: &[u8],
        _cancel: &AtomicBool,
        _on_line: impl FnMut(&str) -> ControlFlow<()>,
    ) -> Result<StreamOutcome, TransportError> {
        Err(TransportError::Connect {
            attempts: 0,
            message: WASM_SYNC_BLOCKING_UNSUPPORTED.to_string(),
        })
    }

    /// 真正的 wasm 实现：建连接 + 退避重试（复用 native 同一份
    /// `Backoff::delay` 计算）+ 流式读 + 分帧，全程 `async`。
    pub async fn post_stream_async(
        &self,
        url: &str,
        api_key: &str,
        body: &[u8],
        cancel: &AtomicBool,
        on_line: impl FnMut(&str) -> ControlFlow<()>,
    ) -> Result<StreamOutcome, TransportError> {
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            if cancel.load(Ordering::Relaxed) {
                return Ok(StreamOutcome::Cancelled);
            }
            let controller = AbortController::new().map_err(|e| TransportError::Connect {
                attempts: attempt,
                message: describe_js_error(&e),
            })?;
            match attempt_fetch(url, api_key, body, &controller).await {
                Ok(response) if response.status() >= 400 => {
                    let status = response.status();
                    let body = read_bounded_error_body(&response).await;
                    return Err(TransportError::Http { status, body });
                }
                Ok(response) => {
                    let source = match response_stream_source(
                        response,
                        controller,
                        cancel,
                        self.cancel_poll_interval,
                    ) {
                        Ok(source) => source,
                        Err(message) => return Ok(StreamOutcome::Broken(message)),
                    };
                    return Ok(drive_stream(source, cancel, on_line).await);
                }
                Err(message) => {
                    if attempt >= self.backoff.max_attempts {
                        return Err(TransportError::Connect {
                            attempts: attempt,
                            message,
                        });
                    }
                    sleep_cancelable_async(
                        self.backoff.delay(attempt),
                        cancel,
                        self.cancel_poll_interval,
                    )
                    .await;
                }
            }
        }
    }

    /// 与 native 签名逐字相同，同样的原因不真的发请求，见模块文档。
    pub fn upload_image(
        &self,
        _base_url: &str,
        _api_key: &str,
        _image: ImageUpload<'_>,
    ) -> Result<String, UploadError> {
        Err(UploadError::Network {
            message: WASM_SYNC_BLOCKING_UNSUPPORTED.to_string(),
        })
    }

    /// 真正的 wasm 实现，转发到 [`crate::fetch_upload::send`]。
    pub async fn upload_image_async(
        &self,
        base_url: &str,
        api_key: &str,
        image: ImageUpload<'_>,
    ) -> Result<String, UploadError> {
        crate::fetch_upload::send(base_url, api_key, image).await
    }
}

/// native `backoff::sleep_cancelable` 的 async 版本：同样每 `tick` 查一次
/// `cancel`，但用 `setTimeout` 让出线程而不是 `std::thread::sleep`——浏览器
/// 单线程模型下没有「挂起当前线程」这回事，`Backoff::delay` 算出来的目标
/// 时长是两边唯一共用的部分（纯计算，见 `backoff.rs` 模块文档）。
async fn sleep_cancelable_async(dur: Duration, cancel: &AtomicBool, tick: Duration) {
    let mut remaining = dur;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let step = remaining.min(tick);
        delay_ms(step.as_millis() as i32).await;
        remaining = remaining.saturating_sub(step);
    }
}
