//! 集成测试共用的假 SSE 服务器 + `RunnerCtx` 装配——跟
//! `agent-runtime/tests/support/mod.rs` 同一个手法（那边的文档注释记了为什么
//! 手写 `TcpListener`：只读到 `\r\n\r\n` + `Content-Length`，响应用
//! `Connection: close` + 断连表示 body 结束）。**这是独立的一份拷贝，不是
//! 依赖复用**：`agent-runtime` 的 `tests/support` 是它自己测试二进制私有的
//! 模块，没有导出成库，`agent-cli` 这边要用同一个假服务器手法只能照抄一份
//! ——跟 `agent-runtime/tests/support/mod.rs` 自己也是「照抄
//! `agent-transport/tests/fake_sse.rs`」同一个先例，本仓已经这么做过两次了。
//!
//! 这份拷贝只留 `agent-cli` 侧集成测试真用到的部分（`Sse` / `HangAfterHeaders`
//! 两种脚本化响应），没有搬 `agent-runtime` 那边超时测试专用的东西。
#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_core::SessionConfig;
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::Client;

fn drain_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return; // 对端提前断开
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
}

fn write_sse_headers(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
        .unwrap();
    stream.flush().unwrap();
}

/// 一个连接该怎么应答。
pub enum ScriptedResponse {
    /// 写响应头 + 逐行 SSE 数据，再正常关闭连接。
    Sse(Vec<&'static str>),
    /// 只写响应头，之后长时间不发任何数据也不关闭——模拟「服务端还在，但
    /// 没数据」，配合测试线程延迟置位取消标志，模拟 Ctrl-C 打断一次还在飞的
    /// `CallProvider`。
    HangAfterHeaders,
}

/// 按顺序应答一串脚本化响应：第 N 次连接对应 `responses[N]`。
///
/// **跟 `agent-runtime/tests/support/mod.rs` 不同的一点**：那边每个测试的
/// 脚本要么全是 `HangAfterHeaders`、要么全是正常 `Sse`，一个 accept 循环
/// 线程顺序处理够用。这边 `cancel_flow.rs` 要在同一个服务器里混一次
/// `HangAfterHeaders`（模拟被取消那轮）紧跟着一次正常 `Sse`（模拟下一轮
/// 真的答完）——如果还是「accept 循环线程自己顺序处理每个连接」，处理第一个
/// 连接时那 5 秒 `sleep` 会堵住循环走到 `accept()` 第二个连接，第二轮请求会
/// 平白多等将近 5 秒（TCP 三次握手能提前完成，但服务器不会开始写数据）。
/// 这里改成每个连接一个独立线程处理响应，`accept()` 循环本身不被
/// 单个连接的处理逻辑卡住——真实的并发 HTTP 服务器就是这么做的，不是
/// 这条测试凭空发明的技巧。
pub fn spawn_scripted_server(responses: Vec<ScriptedResponse>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for resp in responses {
            let Ok((mut stream, _)) = listener.accept() else { return };
            std::thread::spawn(move || {
                drain_request(&mut stream);
                write_sse_headers(&mut stream);
                match resp {
                    ScriptedResponse::Sse(lines) => {
                        for line in lines {
                            let _ = stream.write_all(line.as_bytes());
                            let _ = stream.write_all(b"\n");
                        }
                        let _ = stream.flush();
                    }
                    ScriptedResponse::HangAfterHeaders => {
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
            });
        }
    });
    port
}

/// 每个用例一个独立临时目录，不清理——OS/CI 环境自行回收。
pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-cli-it-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 装一个指向本地假服务器的 `RunnerCtx`：provider 用 DeepSeek（跟
/// `agent-runtime` 的测试同一个取舍——已经有录制帧验证过 wire 形状的那家）。
pub fn build_ctx(port: u16, root: &std::path::Path) -> RunnerCtx {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        agent_transport::Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    let fs = ToolExecutor::new(root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };

    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        ToolTable::builtin(),
        Vec::new(),
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_ev| {}),
    )
}

/// 跟 [`build_ctx`] 一样，只是工具表换成 [`ToolTable::with_shell`]——027 的
/// 屏障验收（`/undo` 撞上 `shell/exec`）要用到真的会执行的不可逆工具。
pub fn build_ctx_with_shell(port: u16, root: &std::path::Path) -> RunnerCtx {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        agent_transport::Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    let fs = ToolExecutor::new(root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };

    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        ToolTable::with_shell(),
        Vec::new(),
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_ev| {}),
    )
}
