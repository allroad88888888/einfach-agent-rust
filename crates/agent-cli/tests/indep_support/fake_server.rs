//! 手写 `TcpListener` 假 SSE 服务器（照抄 `agent-transport/tests/fake_sse.rs`
//! 的手法：手写 HTTP/1.1，只读到 `\r\n\r\n` 再按 `Content-Length` 吃请求体，
//! `Connection: close` + 断连表示响应体结束）。
//!
//! 跟 `fake_sse.rs` 不同的地方：这里要接住**一整个会话**的多次请求，按第 N 次
//! 请求收到的顺序回不同的脚本响应，并把每次收到的请求体原样存下来给测试断言
//! ——这是“下一轮 prompt 不含被撤销/取消轮内容”的黑盒证法。
//!
//! accept 循环放在独立线程里、每个连接再各开一个线程处理，不会因为某个连接
//! 卡住（比如故意慢吞吞不发数据模拟 022 说的“服务端还在但没数据”）而挡住
//! 下一个连接被接受——取消测试要用到这个特性：取消之后立刻会有新请求进来。

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
    /// 先发 `first`，睡 `stall` 模拟“流卡住了”，睡醒后尝试发 `rest`（这时
    /// 客户端多半已经因为取消标志断开，发送失败就静默忽略——测试不关心
    /// 这条连接最终怎么收场，只关心取消发生前收到的那部分）。
    StallThenFinish { first: String, stall: Duration, rest: String },
}

pub struct FakeServer {
    port: u16,
    bodies: Arc<Mutex<Vec<String>>>,
    stop: Arc<AtomicBool>,
}

impl FakeServer {
    /// 在 `127.0.0.1:0`（操作系统分配的空闲端口）上起服务器，按顺序回
    /// `scripts` 里的响应；请求数超过脚本条数时重复最后一条脚本。
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
                        let bodies = Arc::clone(&bodies_bg);
                        let scripts = Arc::clone(&scripts);
                        thread::spawn(move || handle_connection(stream, &bodies, &scripts));
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        FakeServer { port, bodies, stop }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// 迄今收到的全部请求体，按到达顺序排列（第 N 个就是第 N 次网络请求）。
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
    let body = read_request_body(&mut stream);
    // 用“推进 bodies 之后的新长度 - 1”当脚本下标，保证下标严格对应
    // “这是第几个真正到达的请求”，不受 accept 顺序和处理线程调度的影响。
    let idx = {
        let mut guard = bodies.lock().unwrap();
        guard.push(body);
        guard.len() - 1
    };

    match scripts.get(idx).or_else(|| scripts.last()) {
        Some(Script::Immediate(sse_body)) => write_sse_response(&mut stream, sse_body),
        Some(Script::StallThenFinish { first, stall, rest }) => {
            write_sse_headers(&mut stream);
            let _ = stream.write_all(first.as_bytes());
            let _ = stream.flush();
            thread::sleep(*stall);
            let _ = stream.write_all(rest.as_bytes());
            let _ = stream.flush();
        }
        None => write_sse_response(&mut stream, "data: [DONE]\n\n"),
    }
}

/// 读一个请求到「空行」为止，再按 `Content-Length` 吃掉请求体，返回文本。
fn read_request_body(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream for reading"));
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return String::new();
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
    String::from_utf8_lossy(&body).into_owned()
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
