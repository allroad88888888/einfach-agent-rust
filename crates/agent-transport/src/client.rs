//! 阻塞流式 HTTP 客户端。**只支持流式请求**——见 `lib.rs` 顶部的宣告。
//!
//! 退避只包住「连接期」：DNS、拒连、握手（这些在 ureq 里都体现为
//! `ureq::Error::Transport`）。一旦请求字节送到了服务端——无论接下来是收到
//! 响应头还是等响应头等到超时——退避就到头了：见下面
//! [`is_response_wait_failure`] 的说明，这是 023 对 022 的一处修正。
//! 错误状态码交给上层用 `Provider::classify` 分类，不在这里重试；200 就进
//! [`read_loop`]，之后的一切都不再重试。

use std::io::Read;
use std::ops::ControlFlow;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use crate::backoff::{self, Backoff};
use crate::read_loop;
use crate::{StreamOutcome, TransportError};

/// 建立连接允许的总耗时（DNS + TCP + TLS 握手）。跟等响应头无关——等响应头
/// 现在归 [`DEFAULT_SOCKET_TIMEOUT`] 管，两者是不同阶段。
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// socket 的单次 read timeout。**不是轮询节奏**——取消标志的响应速度现在由
/// [`read_loop`] 里读线程与主流程之间的 `recv_timeout`（见
/// [`DEFAULT_CANCEL_POLL_INTERVAL`]）决定，跟这个值完全解耦。这个值同时套在
/// 「等响应头」和「读流式 body」两个阶段（ureq 建 Agent 时把它设成 socket 的
/// 读超时，之后所有 read() 系统调用都受它约束）；它现在只是「连接建立起来了
/// 但对面再也不吭声」这种死流的**最终兜底**，超时就判 [`StreamOutcome::Broken`]
/// 收场，不会无限期占着一个读线程。
///
/// **这是 023 修的一个真实事故**：这里原来是 500ms（跟旧版「短超时轮询取消
/// 标志」的设计耦合在一起，见 022 的实做记录）。Kimi 一类 API 的首字节常态
/// 超过 500ms（实测报错 `Error encountered in the status line: timed out
/// reading response`），500ms 把这种正常的慢首字节直接当成「连接失败」，
/// 退避重试 3 次全灭。**为什么不能靠重试兜**：状态线超时时请求字节已经送到
/// 服务端了，服务端很可能已经在生成——本地断开重发 = 服务端两次生成、
/// 两次计费。真正的修法是等够，不是猜一个更大的超时再重试。60s 对首字节慢
/// 的家绰绰有余，又不至于让一个真正死掉的连接占着资源无限期。
const DEFAULT_SOCKET_TIMEOUT: Duration = Duration::from_secs(60);

/// 主流程轮询取消标志的节奏——传给 [`read_loop::run`] 当 `recv_timeout` 的
/// 参数。跟 [`DEFAULT_SOCKET_TIMEOUT`] 彻底解耦：取消的响应速度只取决于这个
/// 值，不管 socket 那边设的是 500ms 还是 60s。
const DEFAULT_CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// 错误响应体最多读这么多字节——限长，不把整个畸形响应吃进内存。
const MAX_ERROR_BODY: u64 = 8 * 1024;

pub struct Client {
    agent: ureq::Agent,
    backoff: Backoff,
    cancel_poll_interval: Duration,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self::with_config(DEFAULT_CONNECT_TIMEOUT, DEFAULT_CANCEL_POLL_INTERVAL, Backoff::default())
    }

    /// 供测试/特殊部署调「连接超时」「取消轮询节奏」与退避节奏——默认值走
    /// [`Self::new`]。`cancel_poll_interval` 只影响取消标志被发现的延迟上界
    /// （`read_loop` 里 `recv_timeout` 的参数），**不影响** socket 的读超时——
    /// 那个固定是 [`DEFAULT_SOCKET_TIMEOUT`]，是死流的最终兜底，不给外部调，
    /// 调小了就是把 022 那次事故的病根改个数字重新引入一遍。
    pub fn with_config(connect_timeout: Duration, cancel_poll_interval: Duration, backoff: Backoff) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(connect_timeout)
            .timeout_read(DEFAULT_SOCKET_TIMEOUT)
            .build();
        Client { agent, backoff, cancel_poll_interval }
    }

    /// 流式 POST。**一律不重试已经开始的流**：`Ok` 之后的行为交给
    /// [`read_loop::run`]；`Err` 只可能来自真正的连接期失败（拒连、握手超时、
    /// 或非 200 响应）——状态行等待超时不在这条路径上，见
    /// [`is_response_wait_failure`]。
    ///
    /// `cancel` 由调用方共享持有——置位后读循环会在下一次 poll 醒来时停止，
    /// 连接直接丢弃，不做优雅关闭。
    pub fn post_stream(
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
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(StreamOutcome::Cancelled);
            }
            match self.try_connect(url, api_key, body) {
                Ok(resp) => {
                    return Ok(read_loop::run(resp.into_reader(), cancel, self.cancel_poll_interval, on_line));
                }
                Err(ConnectAttemptError::Http { status, body }) => {
                    return Err(TransportError::Http { status, body });
                }
                // 状态行阶段出的错（含超时）：请求已经完整送到服务端了，不是
                // 「建连接失败」，不能退避重试——退避等于让服务端可能已经在
                // 生成的响应被触发第二遍。归到跟「流中途断」一样的待遇，直接
                // 当一次 Broken 上抛，不消耗退避次数。
                Err(ConnectAttemptError::ResponseWaitBroken(message)) => {
                    return Ok(StreamOutcome::Broken(message));
                }
                Err(ConnectAttemptError::Connect(message)) => {
                    if attempt >= self.backoff.max_attempts {
                        return Err(TransportError::Connect { attempts: attempt, message });
                    }
                    backoff::sleep_cancelable(self.backoff.delay(attempt), cancel);
                }
            }
        }
    }

    fn try_connect(&self, url: &str, api_key: &str, body: &[u8]) -> Result<ureq::Response, ConnectAttemptError> {
        match self
            .agent
            .post(url)
            .set("Authorization", &format!("Bearer {api_key}"))
            .set("Content-Type", "application/json")
            .set("Accept", "text/event-stream")
            .send_bytes(body)
        {
            Ok(resp) => Ok(resp),
            // 收到了响应头，只是状态码 >= 400——这不是连接失败，是这家的
            // 明确答复，不退避。分类交给上层的 `Provider::classify`。
            Err(ureq::Error::Status(status, resp)) => {
                Err(ConnectAttemptError::Http { status, body: read_bounded(resp) })
            }
            Err(ureq::Error::Transport(t)) if is_response_wait_failure(&t) => {
                Err(ConnectAttemptError::ResponseWaitBroken(t.to_string()))
            }
            // DNS、拒连、握手失败——真的还没建上连接，可以退避重试。
            Err(ureq::Error::Transport(t)) => Err(ConnectAttemptError::Connect(t.to_string())),
        }
    }
}

/// 判断一个 `ureq::Transport` 错误是不是发生在「请求已经发完、正在等状态行」
/// 这个阶段——包括纯粹的超时，也包括这个阶段里任何别的 IO 错误（连接被对端
/// 重置等）：两者的共同点都是请求字节已经交给了服务端，退避重试的安全前提
/// （「还没让服务端做任何事」）不成立。
///
/// ureq 内部给这个阶段的错误统一打上 `"the status line"` 的上下文（见其
/// `src/response.rs` 里 `read_next_line(&mut stream, "the status line")` 的
/// 调用点，以及 `Transport` 顶层因为经过 `From<io::Error>` 二次包装，
/// `message()` 字段在最外层是 `None`——这个上下文只会出现在完整的 `Display`
/// 输出里）。这是 ureq 唯一暴露出来的信号，本质是拿它的错误措辞当契约；
/// 库内部措辞变了这里要跟着改，回归靠 `tests/fake_sse.rs` 里
/// `slow_status_line_is_tolerated_not_retried` 那条盯着。
fn is_response_wait_failure(t: &ureq::Transport) -> bool {
    t.kind() == ureq::ErrorKind::Io && t.to_string().contains("the status line")
}

enum ConnectAttemptError {
    Http { status: u16, body: String },
    Connect(String),
    /// 见 [`is_response_wait_failure`]：状态行阶段的错，请求已送达，不重试。
    ResponseWaitBroken(String),
}

fn read_bounded(resp: ureq::Response) -> String {
    let mut buf = Vec::new();
    let _ = resp.into_reader().take(MAX_ERROR_BODY).read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}
