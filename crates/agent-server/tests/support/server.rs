//! 手写 `TcpListener` 假 SSE 服务器。手法照抄 `agent-cli/tests/indep_support/
//! fake_server.rs`（那份的模块文档记了为什么手写：只读到 `\r\n\r\n` 再按
//! `Content-Length` 吃请求体，`Connection: close` + 断连表示响应体结束）——
//! 跟那份不同的地方只有一点：这里额外需要「一个服务器同时接住两个 session
//! 各自的并发连接」（两个 session 并行对话的验收要求），accept 循环非阻塞
//! + 每个连接各开一个线程处理，天然满足。
#![allow(dead_code)] // 每个测试二进制各自 `mod support;`，不是每个都用到全部变体。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// 一次请求该怎么回。
pub enum Script {
    /// 一次性把整段 SSE body 发完，然后关闭连接。
    Immediate(String),
    /// 只写响应头，之后长时间不发任何数据也不关闭——模拟「服务端还在，但
    /// 没数据」，供取消测试复用（跟 `agent-runtime/tests/support`
    /// 的 `HangAfterHeaders` 同一个手法）。
    HangAfterHeaders,
}

pub struct FakeServer {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FakeServer {
    /// 在 `127.0.0.1:0` 上起服务器，按顺序回 `scripts` 里的响应；请求数超过
    /// 脚本条数时重复最后一条脚本。
    pub fn start(scripts: Vec<Script>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
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
                        // **必须清掉 O_NONBLOCK**（issue 077）：BSD/macOS 上 accept
                        // 出来的 socket 继承 listener 的非阻塞标志（Linux 不继承）。
                        // 不清的话，下面那个连接线程里的「阻塞式」读会在请求字节还
                        // 没落地时立刻拿到 `WouldBlock`，被 `read_request_body` 当成
                        // 「对面没发请求」——于是记一条空请求、照样把脚本应答写完再
                        // 关连接，客户端那条真请求原地撞 RST。
                        // 先例：`agent-transport/tests/fake_sse.rs` 早就这么写了。
                        let _ = stream.set_nonblocking(false);
                        let bodies = Arc::clone(&bodies_bg);
                        let scripts = Arc::clone(&scripts);
                        thread::spawn(move || handle_connection(stream, &bodies, &scripts));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });

        FakeServer { port, bodies, stop }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }

    /// 迄今收到的全部请求体，按到达顺序排列。
    pub fn bodies(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    pub fn request_count(&self) -> usize {
        self.bodies.lock().unwrap().len()
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn handle_connection(mut stream: TcpStream, bodies: &Mutex<Vec<String>>, scripts: &[Script]) {
    // 没带请求的连接**不记账、也不消耗脚本槽位**：`request_count()` 数的是
    // HTTP 请求，不是 TCP 连接（issue 077）。
    let Some(body) = read_request_body(&mut stream) else { return };
    let idx = {
        let mut guard = bodies.lock().unwrap();
        guard.push(body);
        guard.len() - 1
    };

    match scripts.get(idx).or_else(|| scripts.last()) {
        Some(Script::Immediate(sse_body)) => write_sse_response(&mut stream, sse_body),
        Some(Script::HangAfterHeaders) | None => {
            write_sse_headers(&mut stream);
            // 后台线程，测试断言完就结束——挂着的连接交给进程退出收场
            // （`agent-runtime/tests/support` 与 `agent-cli/tests/indep_support`
            // 的既有权衡）。
            thread::sleep(Duration::from_secs(3));
        }
    }
}

/// 读一个完整的 HTTP 请求。**读不到请求返回 `None`**——「对面没发请求」和
/// 「请求体是空串」是两件事，混成同一个返回值就是 issue 077 那条假红的另一半。
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
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; content_length];
    let _ = reader.read_exact(&mut body);
    Some(String::from_utf8_lossy(&body).into_owned())
}

fn write_sse_headers(stream: &mut TcpStream) {
    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n");
    let _ = stream.flush();
}

fn write_sse_response(stream: &mut TcpStream, body: &str) {
    write_sse_headers(stream);
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
}
