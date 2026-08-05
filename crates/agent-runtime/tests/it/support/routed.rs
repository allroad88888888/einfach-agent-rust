//! **并发**的假 SSE 服务器：按请求体内容路由，每条连接一个线程。
//!
//! `super::spawn_scripted_server` 是「第 N 次连接对应脚本第 N 条」，而且一条
//! 连接服务完才 accept 下一条——029 的两条验收它都做不到：
//!
//! 1. **按 task 路由**：两个子 agent 的请求谁先到达是不确定的（它们真的并行），
//!    「第 N 次连接」这个身份不再稳定。改成看请求体里有什么（子 agent 的第一条
//!    user 消息就是它的 task 文本），身份跟到达顺序解耦。
//! 2. **时间上重叠**：串行 accept 的服务器会把并行的客户端排成队，然后「两个子
//!    agent 的调用重叠了吗」这条断言测的就是服务器而不是泵。每条连接一个线程，
//!    重叠才可能发生，断言才有意义。
//!
//! 每次服务都记一条 [`Call`]（匹配到哪条路由、请求体、收到/答完的时刻），
//! 测试据此断言重叠、断言 prompt 里有没有某段内容。

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 一条路由：请求体里出现 `needle` 就用这条应答。
///
/// **按声明顺序首次匹配**，所以越具体的 needle 越要排在前面；`needle: ""`
/// 匹配一切，当兜底用。
pub struct Route {
    pub needle: &'static str,
    /// 收到请求之后先等这么久再开始应答——制造「一个慢一个快」。
    pub delay: Duration,
    pub status: u16,
    pub lines: Vec<String>,
}

impl Route {
    pub fn sse(needle: &'static str, lines: Vec<impl Into<String>>) -> Self {
        Route {
            needle,
            delay: Duration::ZERO,
            status: 200,
            lines: lines.into_iter().map(Into::into).collect(),
        }
    }

    /// 非 200：写完状态行和一小段 JSON 体就断开（`TransportError::Http`）。
    pub fn http_error(needle: &'static str, status: u16, body: &str) -> Self {
        Route { needle, delay: Duration::ZERO, status, lines: vec![body.to_string()] }
    }

    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }
}

/// 一次被服务过的请求。
#[derive(Clone)]
pub struct Call {
    pub needle: &'static str,
    pub body: String,
    /// 请求体读完的时刻（= 服务端「看到」这次调用）。
    pub start: Instant,
    /// 应答写完的时刻。
    pub end: Instant,
}

pub struct RoutedServer {
    pub port: u16,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl RoutedServer {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let routes = Arc::new(routes);
        let calls: Arc<Mutex<Vec<Call>>> = Arc::new(Mutex::new(Vec::new()));

        let calls_bg = Arc::clone(&calls);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let routes = Arc::clone(&routes);
                let calls = Arc::clone(&calls_bg);
                // 每条连接一个线程——这正是「时间上重叠」得以发生的地方。
                thread::spawn(move || serve(stream, &routes, &calls));
            }
        });

        RoutedServer { port, calls }
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn bodies(&self) -> Vec<String> {
        self.calls().into_iter().map(|c| c.body).collect()
    }

    pub fn call(&self, needle: &str) -> Option<Call> {
        self.calls().into_iter().find(|c| c.needle == needle)
    }

    /// 这两条路由被服务的时间区间**有交叠**吗——并行的证据。
    pub fn overlapped(&self, a: &str, b: &str) -> bool {
        let (Some(a), Some(b)) = (self.call(a), self.call(b)) else { return false };
        a.start < b.end && b.start < a.end
    }
}

fn serve(mut stream: TcpStream, routes: &[Route], calls: &Mutex<Vec<Call>>) {
    let body = read_request(&mut stream);
    let start = Instant::now();
    let Some(route) = routes.iter().find(|r| body.contains(r.needle)) else {
        // 没有路由认领：直接断开，客户端会看到一个说得清的传输错误。
        return;
    };

    if route.status == 200 {
        // **先写响应头，再等 `delay`**：延迟要落在「流上还没有数据」那一段，而不是
        // 「连响应头都还没到」——客户端只有进了读循环才会轮询取消标志
        // （`agent-transport::read_loop`），延迟在响应头之前的话，取消测试会一路
        // 阻塞在 ureq 的 `call()` 里，测出来的是「等满了脚本的睡眠」而不是取消。
        // 顺带这也更像真实的 provider：TTFB 之后才是逐 token 的流。
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
            .unwrap();
        let _ = stream.flush();
        thread::sleep(route.delay);
        for line in &route.lines {
            let _ = stream.write_all(line.as_bytes());
            let _ = stream.write_all(b"\n");
        }
    } else {
        thread::sleep(route.delay);
        let payload = route.lines.first().map_or("{}", String::as_str);
        let head = format!(
            "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            route.status,
            payload.len(),
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(payload.as_bytes());
    }
    let _ = stream.flush();
    let end = Instant::now();

    calls.lock().unwrap().push(Call { needle: route.needle, body, start, end });
}

/// 读一次 HTTP 请求，返回请求体（按 `Content-Length`）。
fn read_request(stream: &mut TcpStream) -> String {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
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
