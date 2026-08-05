//! **并发**的假上游 SSE 服务器：按请求体内容路由，每条连接一个线程——手法照
//! `agent-runtime/tests/support/routed.rs`（029 已验证过的技巧：按 task 文本路由、
//! 一条连接一个线程才谈得上「时间上重叠」），这里为 034 的 spawn-over-HTTP
//! 集成测试独立实现（跨 crate 抄手法不抄文件）。
//!
//! 比 029 那份多一样东西：[`Route::paced`]——一条路由可以分**多段**发，每段各自
//! 带一个「写完上一段之后再等多久」的延迟。029 只需要证明「两个子 agent 的服务
//! 区间有重叠」（server 端时间戳），034 的验收原文更进一步，要看到「SSE 帧里
//! 两个子 agent 的归属交错出现」——这要求两个子 agent 的流式增量在 wall-clock
//! 上真的交替抵达，不只是各自的请求处理窗口有重叠，`paced` 就是让这件事可控地
//! 发生的手段。

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// 一条路由要发的一段：写完上一段之后先等 `delay`，再写 `line`（+ 换行）。
pub struct Segment {
    pub delay: Duration,
    pub line: String,
}

/// 一条路由：请求体里出现 `needle` 就用它应答。**按声明顺序首次匹配**，越
/// 具体的 needle 越要排在前面，`needle: ""` 匹配一切，当兜底用。
pub struct Route {
    pub needle: &'static str,
    pub segments: Vec<Segment>,
}

impl Route {
    /// 一次性写完，段间不等待——大多数路由用这个。
    pub fn sse(needle: &'static str, lines: Vec<impl Into<String>>) -> Self {
        Route {
            needle,
            segments: lines
                .into_iter()
                .map(|line| Segment {
                    delay: Duration::ZERO,
                    line: line.into(),
                })
                .collect(),
        }
    }

    /// 分段带节奏地写——每个 `(delay, line)` 里的 `delay` 是「写完上一段之后
    /// 再等多久」。用来让两条并发路由的多个 chunk 在 wall-clock 上真的交替
    /// 抵达客户端，而不是各自成块。
    pub fn paced(needle: &'static str, segments: Vec<(Duration, &str)>) -> Self {
        Route {
            needle,
            segments: segments
                .into_iter()
                .map(|(delay, line)| Segment {
                    delay,
                    line: line.to_string(),
                })
                .collect(),
        }
    }
}

/// 一次被服务过的请求：匹配到哪条路由、请求体、服务区间（收到/答完的时刻）。
#[derive(Clone)]
pub struct Call {
    pub needle: &'static str,
    pub body: String,
    pub start: Instant,
    pub end: Instant,
}

pub struct RoutedServer {
    pub port: u16,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl RoutedServer {
    pub fn start(routes: Vec<Route>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind routed server");
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

    pub fn endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/chat/completions", self.port)
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    pub fn call(&self, needle: &str) -> Option<Call> {
        self.calls().into_iter().find(|c| c.needle == needle)
    }

    /// 这两条路由被服务的时间区间**有交叠**吗——并行的证据。
    pub fn overlapped(&self, a: &str, b: &str) -> bool {
        let (Some(a), Some(b)) = (self.call(a), self.call(b)) else {
            return false;
        };
        a.start < b.end && b.start < a.end
    }
}

fn serve(mut stream: TcpStream, routes: &[Route], calls: &Mutex<Vec<Call>>) {
    let body = read_request(&mut stream);
    let start = Instant::now();
    let Some(route) = routes.iter().find(|r| body.contains(r.needle)) else {
        return; // 没有路由认领：直接断开。
    };

    let _ = stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
    );
    let _ = stream.flush();
    for seg in &route.segments {
        if !seg.delay.is_zero() {
            thread::sleep(seg.delay);
        }
        let _ = stream.write_all(seg.line.as_bytes());
        let _ = stream.write_all(b"\n");
        let _ = stream.flush();
    }
    let end = Instant::now();

    calls.lock().unwrap().push(Call {
        needle: route.needle,
        body,
        start,
        end,
    });
}

/// 读一次 HTTP 请求，返回请求体（按 `Content-Length`）。
fn read_request(stream: &mut TcpStream) -> String {
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
