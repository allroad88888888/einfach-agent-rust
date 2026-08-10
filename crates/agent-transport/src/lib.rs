//! 阻塞 HTTP transport（issue 022）：ureq 2.x 做流式 POST + 连接期退避 + jitter，
//! 外加 `providers.toml` 解析。**这是唯一允许依赖 `ureq` 的 crate**（红线 7 的
//! 镜像约束——check-invariants.sh 只查 agent-core/agent-store 不许出现它，
//! 「只此一家」这条靠人读 Cargo.toml：全仓 `grep ureq` 应该只命中这个目录）。
//!
//! # 只支持流式请求
//!
//! [`client::Client::post_stream`] 是这个 crate 唯一的请求方法。这不是遗漏，
//! 是 025「实做记录」里记的第二笔待清账：`agent_providers::Encoded::body` 恒
//! 把 `stream` 设成 `true`（DeepSeek 的 `encode` 写死，usage 只有开流式才随
//! 尾帧回来，见 `deepseek::encode::body`），而 `Provider::decode` 吃的是非流式
//! 响应体——两者暂不矛盾（`decode` 服务录制帧测试与未来的非流式兜底），但接缝
//! 上确实没有「要不要流」的开关。022/012 的真实接线只走流式这一条路，这里就
//! 只实现这一条路：**要加非流式 POST，另开一个方法，不要改这个的语义**。
//!
//! # 错误分类不在这里
//!
//! 非 200 响应只把状态码 + 响应体（限长）打包成 [`TransportError::Http`] 交
//! 给上层——分类成 `agent_core::ErrorClass` 要调具体 provider 的
//! `Provider::classify`（各家状态码分配不一致，见 PROVIDERS.md §四），
//! transport 不知道自己在跟哪家说话，也不该猜。
//!
//! # Ctrl-C 中断阻塞流（native 侧）
//!
//! ureq 的阻塞 `read` 没有外部中断句柄。022 的第一版办法是给 socket 设短 read
//! timeout 直接当轮询节奏用；023 发现这个办法在慢首字节的家（Kimi）上会把
//! 「服务端还没吐字节」误判成「连接死了」（连接期退避重试 3 次全灭），改成
//! 读线程 + `mpsc::sync_channel`：socket 的 read timeout 放宽到 60s，只做死流
//! 的最终兜底；取消标志的轮询节奏搬到主流程的 `recv_timeout`，跟 socket 超时
//! 解耦。办法与取舍见 [`client`] 顶部的事故记录、[`read_loop`] 模块文档，以及
//! docs/issues/022-first-provider.md §注意（原始设计取舍，023 延续同一个
//! 「不优雅但可测」的取舍，只是拆成了两个独立的旋钮）。
//!
//! # wasm 侧（issue 113）
//!
//! 上面那整套读线程 + 双超时旋钮，存在的唯一理由是 ureq 的阻塞 `read` 没有
//! 外部中断句柄。浏览器的 `fetch` 原生给流式响应体（`ReadableStream`），
//! `AbortController` 原生就是那个中断句柄——`read_loop.rs` 那 165 行在 wasm
//! 目标上不需要存在。`wasm32` 目标编的是 [`client`]/[`read_loop`]/[`upload`]/
//! [`config`] 的替身：[`fetch_client`]（`post_stream`/`post_stream_async`/
//! `upload_image`）+ [`fetch_upload`]（图片上传的 fetch 版）。两边共享的部分：
//!
//! - [`backoff`]：`Backoff::delay` 的指数退避计算，纯函数，两边原样复用；
//!   `sleep_cancelable`（真的挂起当前线程）是 native 专属，wasm 侧退避等待
//!   用 `fetch_client` 里自己的 async 定时器，因为浏览器单线程模型下没有
//!   「挂起当前线程」这回事。
//! - [`line_framer`]：按 `\n` 拆行，语义与 `read_loop.rs` 里
//!   `BufReader::read_line` + `trim_end_matches(['\r','\n'])` 逐条对齐。
//! - [`stream_drive`]：`read_loop::run` 的 wasm 对应物，`drive_stream` 配
//!   `ChunkSource` trait 决定字节从哪儿来——wasm 生产代码接
//!   `ReadableStreamDefaultReader`，本 crate 自己的测试
//!   （`framing_parity_tests`）接一个不碰任何 JS 绑定的内存 mock，在 native
//!   目标上把**同一个 `drive_stream` 函数**跑起来，和 `read_loop::run` 喂
//!   同一份字节比对逐行输出——这就是「wasm 分帧与 native 分帧逐字节相同」
//!   的证明方式，细节见 `framing_parity_tests.rs` 顶部注释。
//!
//! `config.rs`（`providers.toml` 解析）**不移植**——浏览器里没有这个文件，
//! 配置来源见 issue 114；`wasm32` 目标不导出 `config` 模块。
//!
//! **上层零改动**：`agent-providers`/`agent-runtime` 看到的 `Client` 方法表
//! （`new`/`with_config`/`post_stream`/`upload_image`）在两个目标上完全相同，
//! 靠 `#[cfg(target_arch = "wasm32")]` 二选一 `pub use`，不是两份并存的类型。

mod backoff;
// `upload` 的类型（`ImageUpload`/`UploadError`/`MAX_IMAGE_BYTES`）与 multipart
// 编码是平台无关的纯逻辑，两边共用；只有本机 `send()`（吃 `ureq::Agent`）
// 在文件内部用 `#[cfg(not(target_arch = "wasm32"))]` 单独包住，wasm 侧的
// `send()` 在 `fetch_upload.rs` 里。模块声明本身不能整体 cfg 到 native——
// 那样 wasm 目标就拿不到 `ImageUpload` 这个类型了。
mod upload;

#[cfg(not(target_arch = "wasm32"))]
mod client;
#[cfg(not(target_arch = "wasm32"))]
pub mod config;
#[cfg(not(target_arch = "wasm32"))]
mod read_loop;

#[cfg(target_arch = "wasm32")]
mod fetch_client;
#[cfg(target_arch = "wasm32")]
mod fetch_request;
#[cfg(target_arch = "wasm32")]
mod fetch_upload;
#[cfg(target_arch = "wasm32")]
mod js_timer;
#[cfg(target_arch = "wasm32")]
mod web_stream_source;

// `line_framer`/`stream_drive` 是 wasm 的分帧核心，同时也是「wasm 分帧与
// native 分帧逐字节相同」这条验收的证明现场——后者要在 native 目标上跑
// `drive_stream` 才能和 `read_loop::run` 同框比较，所以两个模块在
// wasm32 目标**或** `cfg(test)` 下都要编译，不能只挂在 wasm32 上。
#[cfg(any(target_arch = "wasm32", test))]
mod line_framer;
#[cfg(any(target_arch = "wasm32", test))]
mod stream_drive;

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "framing_parity_tests.rs"]
mod framing_parity_tests;

pub use backoff::Backoff;
pub use upload::{ImageUpload, MAX_IMAGE_BYTES, UploadError};

#[cfg(not(target_arch = "wasm32"))]
pub use client::Client;
#[cfg(not(target_arch = "wasm32"))]
pub use config::{ConfigError, DefaultConfig, ProviderConfig, RootConfig, default_provider, load};

#[cfg(target_arch = "wasm32")]
pub use fetch_client::Client;

/// 一次 `post_stream` 读到头的方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamOutcome {
    /// 对端正常关闭连接（读到 EOF）。SSE 协议里真正的「结束」信号是
    /// `data: [DONE]`，那是 `on_line` 回调自己认的——这个变体只说 TCP 层面
    /// 连接关掉了，两者通常前后脚发生但不是一回事。
    Finished,
    /// 取消标志置位，或 `on_line` 返回 `ControlFlow::Break`——两种触发方式
    /// 效果相同：立刻停止读、丢弃连接，不重试、不优雅关闭。
    Cancelled,
    /// 连接中途出现非超时的 IO 错误（对端异常断开、TLS 错误等）。已经吐出去
    /// 的增量收不回来，这里不重试；上层据此知道这一轮响应可能不完整。
    Broken(String),
}

/// `post_stream` 在拿到第一个响应之前失败的两种方式。**不含 `agent_core::
/// ErrorClass`**——分类留给上层调具体 provider 的 `classify`（本文件顶部
/// 「错误分类不在这里」）。两个变体都不含 API key：`Http` 只搬运服务端的
/// 状态码和响应体，`Connect` 只搬运 `ureq` 自己的错误描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransportError {
    /// 连接期重试耗尽（DNS、拒连、握手失败，或迟迟等不到响应头）。
    Connect { attempts: u32, message: String },
    /// 收到了响应头，状态码 `>= 400`。**不退避**——这是这家的明确答复，不是
    /// 传输层的偶发故障；402（余额耗尽）也在这条路径上，绝不能因为混进这里
    /// 就被当成可重试的传输故障对待。
    Http { status: u16, body: String },
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Connect { attempts, message } => {
                write!(f, "连接失败（尝试 {attempts} 次）: {message}")
            }
            TransportError::Http { status, body } => {
                write!(f, "HTTP {status}: {body}")
            }
        }
    }
}

impl std::error::Error for TransportError {}
