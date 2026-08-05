//! 集成测试共用的假 SSE 服务器 + `RunnerCtx` 装配。手写 `TcpListener`
//! 零第三方 HTTP 依赖——手法照抄 `agent-transport/tests/fake_sse.rs`
//! （那边的模块文档记了为什么手写：只读到 `\r\n\r\n` + `Content-Length`，
//! 响应用 `Connection: close` + 断连表示 body 结束）。
//!
//! `dead_code` 允许：每个集成测试二进制各自 `mod support;` 引入本文件一份
//! 拷贝，不是每个二进制都用到全部变体/方法（比如 `timeout.rs` 只用
//! `HangAfterHeaders`）——跟 `agent-tools/tests/support/mod.rs` 同一个取舍。
#![allow(dead_code)]

pub mod mcp;
pub mod routed;

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use agent_core::SessionConfig;
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, RunnerEvent, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

pub fn drain_request(stream: &mut TcpStream) {
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

pub fn write_sse_headers(stream: &mut TcpStream) {
    stream
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
        .unwrap();
    stream.flush().unwrap();
}

/// 一个连接该怎么应答。
pub enum ScriptedResponse {
    /// 写响应头 + 逐行 SSE 数据，再正常关闭连接（`StreamOutcome::Finished`）。
    Sse(Vec<&'static str>),
    /// 只写响应头，之后长时间不发任何数据也不关闭——供超时/取消测试复用，
    /// 模拟「服务端还在，但没数据」（跟 `fake_sse.rs` 的取消测试同一个手法）。
    HangAfterHeaders,
}

/// 按顺序应答一串脚本化响应：第 N 次连接对应 `responses[N]`。多于脚本条数的
/// 连接不会发生——调用方要保证连接次数跟脚本条数对得上（重试次数由
/// `TurnState::max_retries` 控制）。
///
/// 服务器线程不 join——测试断言完就结束，挂着的 `HangAfterHeaders` 连接
/// 交给进程退出收场（`fake_sse.rs` 已经用的同一个权衡）。
pub fn spawn_scripted_server(responses: Vec<ScriptedResponse>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for resp in responses {
            let Ok((mut stream, _)) = listener.accept() else { return };
            drain_request(&mut stream);
            write_sse_headers(&mut stream);
            match resp {
                ScriptedResponse::Sse(lines) => {
                    for line in lines {
                        let _ = stream.write_all(line.as_bytes());
                        let _ = stream.write_all(b"\n");
                    }
                    let _ = stream.flush();
                    // drop(stream)：连接关闭 → 客户端读到 EOF → Finished。
                }
                ScriptedResponse::HangAfterHeaders => {
                    std::thread::sleep(Duration::from_secs(5));
                }
            }
        }
    });
    port
}

/// 每个用例一个独立临时目录，不清理——OS/CI 环境自行回收
/// （跟 `agent-tools/src/exec.rs` 测试的 `temp_root` 同一个取舍）。
pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("agent-runtime-it-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 装一个指向本地假服务器的 `RunnerCtx`：provider 用 DeepSeek（三家里已经有
/// 录制帧验证过 wire 形状的那家，测试直接复用它的形状，不必再摸一遍另外
/// 两家的转义规则）。`cancel_poll_interval` 拉到 50ms——测试要快。
///
/// 返回的 `Rc<RefCell<Vec<RunnerEvent>>>` 收集所有经回调发出的事件：
/// `run_turn` 是同步阻塞的，回调只会在调用 `run_turn` 的这同一个线程上被喊到
/// （IO 线程只经 channel 传数据，不直接碰回调，见 `provider_call` 模块文档），
/// 所以这里不需要 `Arc<Mutex<_>>`。
/// 029 之前的用例只认 `RunnerEvent`：走 `RunnerCtx::new` 那条不带归属的回调，
/// 断言因此一个字不用改（单 agent 时「谁说的」只有一个答案）。
pub fn build_ctx(port: u16, root: &std::path::Path) -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    build_ctx_with(port, root, ToolTable::builtin())
}

/// 带归属的事件收集版：029 的多 agent 用例用它断言「谁说的」。
pub fn build_ctx_agent_aware(
    port: u16,
    root: &std::path::Path,
    tools: ToolTable,
) -> (RunnerCtx, Rc<RefCell<Vec<agent_runtime::AgentEvent>>>) {
    let (ctx, _) = build_ctx_with(port, root, tools);
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let ctx = ctx.with_agent_events(Box::new(move |ev| sink.borrow_mut().push(ev)));
    (ctx, events)
}

/// DeepSeek wire：一条工具调用响应（hop1）。`wire_name` 是**转义后**的工具名
/// （`web:nope/x` → `web_3Anope_2Fx`，见 `agent_providers::wire_name`），
/// `arguments` 是模型写进 `function.arguments` 那个**字符串**的原文
/// （里面的引号要按 JSON 字符串再转义一次）。
pub fn sse_tool_call(call_id: &str, wire_name: &str, arguments: &str) -> ScriptedResponse {
    let line = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"{call_id}","type":"function","function":{{"name":"{wire_name}","arguments":"{arguments}"}}}}]}}}}]}}"#
    );
    ScriptedResponse::Sse(vec![
        Box::leak(line.into_boxed_str()),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
        "data: [DONE]",
    ])
}

/// DeepSeek wire：一条普通 `EndTurn` 文本响应（工具结果回来之后模型收敛）。
pub fn sse_text(text: &str) -> ScriptedResponse {
    let line = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
    );
    ScriptedResponse::Sse(vec![
        Box::leak(line.into_boxed_str()),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":150,"completion_tokens":10,"prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":86}}"#,
        "data: [DONE]",
    ])
}

pub fn build_ctx_with(
    port: u16,
    root: &std::path::Path,
    tools: ToolTable,
) -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff { base: Duration::from_millis(10), max_attempts: 1 },
    );
    let fs = ToolExecutor::new(root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };

    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        tools,
        Vec::new(),
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(move |ev| sink.borrow_mut().push(ev)),
    );
    (ctx, events)
}
