//! 最底层：从一个已连接的 `TcpStream` 上读 HTTP/1.1 响应的状态行 + header 块。
//! 独立手写（不看实现方 `tests/support/http_client.rs`）——只读到空行
//! （`\r\n\r\n`）为止，之后 body 怎么读（`Content-Length` 还是 chunked）由
//! 调用方（`raw_http.rs` / `sse_client.rs`）决定。

#![allow(dead_code)]

use std::io::Read;
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct ResponseHead {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// header 块的原始字节（含状态行，不含结尾的空行），用于「两个 header 逐字节
    /// 在响应头里」这类断言——不经过任何解析器改写。
    pub raw: String,
}

impl ResponseHead {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// 阻塞读到 `\r\n\r\n`（header 块结束）为止，返回解析结果和紧跟在 header 块
/// 之后、这一次 `read` 顺带读到的 body 前缀字节（不能扔掉——TCP 不按 HTTP
/// 报文的语义边界切包，一次 `read` 常常已经带上一部分 body 甚至下一个 chunk）。
pub fn read_head(stream: &mut TcpStream, timeout: Duration) -> (ResponseHead, Vec<u8>) {
    stream
        .set_read_timeout(Some(timeout))
        .expect("set_read_timeout");
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let deadline = Instant::now() + timeout;
    let split = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        if Instant::now() >= deadline {
            panic!(
                "读响应头超时，目前已读到：{:?}",
                String::from_utf8_lossy(&buf)
            );
        }
        match stream.read(&mut chunk) {
            Ok(0) => panic!(
                "连接在读完响应头之前就关闭了，目前已读到：{:?}",
                String::from_utf8_lossy(&buf)
            ),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => panic!("读响应头失败：{e}"),
        }
    };
    let head_bytes = &buf[..split];
    let rest = buf[split + 4..].to_vec();
    let raw = String::from_utf8_lossy(head_bytes).into_owned();
    (parse_head(&raw), rest)
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_head(raw: &str) -> ResponseHead {
    let mut lines = raw.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    // "HTTP/1.1 200 OK"
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    ResponseHead {
        status,
        headers,
        raw: raw.to_string(),
    }
}

/// 阻塞读，尽量填满 `buf`，超时或对端关闭都算「这次读到这里」。给调用方在读
/// body 阶段用。
pub fn read_some(stream: &mut TcpStream, timeout: Duration) -> Option<Vec<u8>> {
    stream
        .set_read_timeout(Some(timeout))
        .expect("set_read_timeout");
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk) {
        Ok(0) => None,
        Ok(n) => Some(chunk[..n].to_vec()),
        Err(_) => Some(Vec::new()), // 超时：这次没读到新数据，但连接还在，交调用方决定要不要重试
    }
}
