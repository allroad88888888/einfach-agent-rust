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
//! # Ctrl-C 中断阻塞流
//!
//! ureq 的阻塞 `read` 没有外部中断句柄。022 的第一版办法是给 socket 设短 read
//! timeout 直接当轮询节奏用；023 发现这个办法在慢首字节的家（Kimi）上会把
//! 「服务端还没吐字节」误判成「连接死了」（连接期退避重试 3 次全灭），改成
//! 读线程 + `mpsc::sync_channel`：socket 的 read timeout 放宽到 60s，只做死流
//! 的最终兜底；取消标志的轮询节奏搬到主流程的 `recv_timeout`，跟 socket 超时
//! 解耦。办法与取舍见 [`client`] 顶部的事故记录、[`read_loop`] 模块文档，以及
//! docs/issues/022-first-provider.md §注意（原始设计取舍，023 延续同一个
//! 「不优雅但可测」的取舍，只是拆成了两个独立的旋钮）。

mod backoff;
mod client;
mod read_loop;
mod upload;

pub mod config;

pub use backoff::Backoff;
pub use client::Client;
pub use config::{ConfigError, DefaultConfig, ProviderConfig, RootConfig, default_provider, load};
pub use upload::{ImageUpload, MAX_IMAGE_BYTES, UploadError};

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
