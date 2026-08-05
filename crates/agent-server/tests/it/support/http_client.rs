//! 手写「假浏览器」：原生 `TcpStream` 发 HTTP/1.1 请求、读响应——issue 031 原文
//! 要求（「假浏览器（原生 TcpStream 客户端）」），跟 `tests/support/server.rs`
//! 手写假 SSE 服务器同一个理由：不额外引入 HTTP 客户端依赖，也逼着测试真的走
//! 一遍 wire 格式（headers、chunked transfer-encoding），而不是只验证 handler
//! 函数的返回值。
#![allow(dead_code)]

use std::io::{BufReader, Read};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::http_chunked::find;
use super::http_request::{connect_and_send, header, read_head, with_private_capability};
pub use super::http_request::{request, request_exact_headers, request_with_headers};
pub use super::http_response::HttpResponse;

/// 打开一条 SSE 连接（不发 body），返回状态行/headers 和一个可以增量
/// `next_event` 的读取器。真实浏览器的 `EventSource` 长这样：一次 `GET`,
/// 连接开着不关,数据一帧一帧地来。
pub fn connect_sse(
    addr: SocketAddr,
    path: &str,
    last_event_id: Option<u64>,
) -> (u16, Vec<(String, String)>, SseReader) {
    let extra: Vec<(&str, String)> = last_event_id
        .map(|id| ("last-event-id", id.to_string()))
        .into_iter()
        .collect();
    let extra_refs: Vec<(&str, &str)> = extra.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let headers = with_private_capability(&extra_refs);
    let mut reader = connect_and_send(addr, "GET", path, &headers, None);
    let (status, headers) = read_head(&mut reader);
    let chunked = header(&headers, "transfer-encoding")
        .map(|v| v.eq_ignore_ascii_case("chunked"))
        .unwrap_or(false);
    (
        status,
        headers,
        SseReader {
            reader,
            chunked,
            raw: Vec::new(),
            decoded: Vec::new(),
            done: false,
        },
    )
}

/// 一条 SSE 事件：`id`（没有 `id:` 行就是 `None`，这个仓库的服务端每一帧都会
/// 带 id，`None` 出现说明协议变了，不该悄悄忽略）+ 拼起来的 `data` 文本。
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub id: Option<u64>,
    pub data: String,
}

pub struct SseReader {
    reader: BufReader<TcpStream>,
    chunked: bool,
    /// chunked 模式下还没解出完整数据块的原始字节；非 chunked 模式不用。
    raw: Vec<u8>,
    /// 已经解出来、还没被切成完整事件消费掉的 SSE 文本字节。
    decoded: Vec<u8>,
    done: bool,
}

impl SseReader {
    /// 等下一条完整事件，最多等 `timeout`。`None` = 超时还没等到（调用方拿这个
    /// 断言「这段时间里不该有事件」），或者连接已经彻底结束。
    pub fn next_event(&mut self, timeout: Duration) -> Option<SseEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(event) = self.try_take_event() {
                return Some(event);
            }
            if self.done || Instant::now() >= deadline {
                return None;
            }
            self.pump();
        }
    }

    /// 从 socket 读一批字节,按 chunked/非 chunked 追加进 `decoded`。读超时
    /// （还没到 `next_event` 的整体 deadline）不算错误,直接返回让上层重试。
    fn pump(&mut self) {
        let mut tmp = [0u8; 4096];
        match self.reader.read(&mut tmp) {
            Ok(0) => self.done = true,
            Ok(n) if self.chunked => {
                self.raw.extend_from_slice(&tmp[..n]);
                self.drain_chunks();
            }
            Ok(n) => self.decoded.extend_from_slice(&tmp[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut => {}
            Err(_) => self.done = true,
        }
    }

    /// 把 `raw` 里凑够的完整 chunk 挪进 `decoded`，不完整的留着等下一次 `pump`。
    fn drain_chunks(&mut self) {
        loop {
            let Some(line_end) = find(&self.raw, b"\r\n") else {
                return;
            };
            let size_str = String::from_utf8_lossy(&self.raw[..line_end]);
            let size_str = size_str.split(';').next().unwrap_or("").trim();
            let Ok(size) = usize::from_str_radix(size_str, 16) else {
                self.done = true; // 解不出块大小：框架跟预期的不一样，别硬扛。
                return;
            };
            let needed = line_end + 2 + size + 2;
            if self.raw.len() < needed {
                return; // 这块还没收全，等下一批字节。
            }
            if size == 0 {
                self.done = true; // 终止块："0\r\n\r\n"。
                self.raw.clear();
                return;
            }
            self.decoded
                .extend_from_slice(&self.raw[line_end + 2..line_end + 2 + size]);
            self.raw.drain(..needed);
        }
    }

    /// SSE 事件以空行（`\n\n`）结尾，注释行（`:` 开头，axum `KeepAlive` 的心跳）
    /// 直接跳过不当成事件。
    fn try_take_event(&mut self) -> Option<SseEvent> {
        loop {
            let sep = find(&self.decoded, b"\n\n")?;
            let raw = self.decoded[..sep].to_vec();
            self.decoded.drain(..sep + 2);
            let text = String::from_utf8_lossy(&raw);
            let mut id = None;
            let mut data = String::new();
            let mut saw_data = false;
            for line in text.split('\n') {
                if let Some(v) = line.strip_prefix("id:") {
                    id = v.trim().parse().ok();
                } else if let Some(v) = line.strip_prefix("data:") {
                    saw_data = true;
                    data.push_str(v.trim());
                }
                // `:` 开头的纯注释行（心跳）——两个字段都不匹配，天然跳过。
            }
            if saw_data {
                return Some(SseEvent { id, data });
            }
            // 纯心跳块，没有 data——继续找下一条。
        }
    }
}
