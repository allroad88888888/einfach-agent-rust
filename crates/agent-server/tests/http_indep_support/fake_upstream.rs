//! 假上游 LLM（DeepSeek 形状的 `/chat/completions`）。跟 `crates/agent-server/
//! tests/support/server.rs` 是同一个手法（手写 `TcpListener`，一个连接一个
//! 线程），但这是独立测试 agent 自己的实现，不是读了那份代码抄的——两份并存
//! 完全正常（两边各自独立验证同一个契约）。
//!
//! 这一层是「假上游」不是「被测的 agent-server HTTP 面」——独测规则允许读
//! `tests/support/` 里非 http 的既有夹具正是为了学这个手法，写自己的版本天经
//! 地义。

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// 一次请求该怎么回。
pub enum Script {
    /// 回一段纯文本回复（一帧 `content` + `finish_reason: stop`，随后 `[DONE]`）。
    Text(String),
    /// 只写响应头就不再发任何数据、也不关闭连接——模拟「上游挂住不回」，供
    /// 宽限取消测试复用。
    Hang,
}

pub struct FakeUpstream {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FakeUpstream {
    /// 在 `127.0.0.1:0` 起服务器，按顺序回 `scripts`；请求数超过脚本条数时
    /// 重复最后一条。
    pub fn start(scripts: Vec<Script>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake upstream");
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).expect("nonblocking accept loop");

        let bodies = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let scripts = Arc::new(scripts);

        let bodies_bg = Arc::clone(&bodies);
        let stop_bg = Arc::clone(&stop);
        thread::spawn(move || {
            loop {
                if stop_bg.load(Ordering::Relaxed) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        // 见 `tests/support/server.rs` 同一处的注释（issue 077）：
                        // BSD/macOS 上 accept 出来的 socket 继承 listener 的
                        // O_NONBLOCK，不清掉就会把「请求字节还在路上」误判成
                        // 「没带请求」。
                        let _ = stream.set_nonblocking(false);
                        let bodies = Arc::clone(&bodies_bg);
                        let scripts = Arc::clone(&scripts);
                        thread::spawn(move || handle_one(stream, &bodies, &scripts));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });

        FakeUpstream { port, bodies, stop }
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }

    /// 迄今收到的全部请求体，按到达顺序。用于「上游请求体不含被退内容」这类
    /// undo 语义断言（027 的证法搬到 HTTP 层）。
    pub fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    pub fn request_count(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
}

impl Drop for FakeUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle_one(mut stream: TcpStream, bodies: &Mutex<Vec<String>>, scripts: &[Script]) {
    // 没带请求的连接不记账、也不消耗脚本槽位（issue 077）。
    let Some(body) = read_request_body(&mut stream) else { return };
    let idx = {
        let mut guard = bodies.lock().unwrap();
        guard.push(body);
        guard.len() - 1
    };

    let picked = match scripts.get(idx) {
        Some(s) => Some(s),
        None => scripts.last(),
    };
    match picked {
        Some(Script::Text(text)) => write_sse(&mut stream, &text_reply(text)),
        Some(Script::Hang) | None => {
            write_headers_only(&mut stream);
            // 挂住不回——够长，测试自己的超时/宽限窗口远小于这个数，进程退出
            // 时这条后台线程直接被回收，不需要更优雅的收尾。
            thread::sleep(Duration::from_secs(20));
        }
    }
}

fn text_reply(text: &str) -> String {
    let content = serde_json::to_string(text).expect("json string");
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{content}}},\"finish_reason\":\"stop\"}}]}}\n\ndata: [DONE]\n\n")
}

/// 读不到请求返回 `None`——跟 `tests/support/server.rs` 同款（issue 077）。
fn read_request_body(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream for reading"));
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return None;
        }
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
    Some(String::from_utf8_lossy(&body).into_owned())
}

fn write_headers_only(stream: &mut TcpStream) {
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
    let _ = stream.flush();
}

fn write_sse(stream: &mut TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}
