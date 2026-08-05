//! 假浏览器的 SSE 客户端：连接 `GET /sessions/:id/events`、增量 dechunk、逐帧
//! 解析 `id:`/`data:`，跳过 `:`开头的注释行（axum 的心跳 `: keep-alive`）。
//! 独立手写，不看实现方的 `http_client.rs`——这正是独立性的一部分。

#![allow(dead_code)]

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::chunked::ChunkedDecoder;
use super::wire::{ResponseHead, read_head, read_some};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub id: Option<u64>,
    /// `data:` 行的原始文本（去掉前缀 `data: ` 和结尾换行），逐字节保留——
    /// 「补发内容与首播逐字节同」这类断言直接比较这个字符串。
    pub data: String,
}

pub struct SseClient {
    stream: TcpStream,
    decoder: ChunkedDecoder,
    /// dechunk 之后、还没被切成完整帧的文本字节。
    text_buf: Vec<u8>,
    pub head: ResponseHead,
}

impl SseClient {
    /// 连接并发 `GET /sessions/:id/events`，`last_event_id` 非空时带上
    /// `Last-Event-ID` 请求头（031 补发协议）。阻塞到读完响应头为止。
    pub fn connect(addr: SocketAddr, session_id: &str, last_event_id: Option<u64>) -> Self {
        let mut stream = TcpStream::connect(addr).unwrap_or_else(|e| panic!("connect {addr} 失败：{e}"));
        let mut req = format!("GET /sessions/{session_id}/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n");
        if let Some(id) = last_event_id {
            req.push_str(&format!("Last-Event-ID: {id}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes()).expect("write SSE request");

        let (head, leftover) = read_head(&mut stream, Duration::from_secs(5));
        let mut decoder = ChunkedDecoder::new();
        decoder.feed(&leftover);
        let text_buf = decoder.take_decoded();
        SseClient { stream, decoder, text_buf, head }
    }

    pub fn status(&self) -> u16 {
        self.head.status
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.head.header(name)
    }

    /// 等下一帧「真事件」（`id:`+`data:`），跳过注释/心跳行。`None` = 在
    /// `timeout` 内没有等到（不代表连接断了——心跳期间没有真事件是正常状态）。
    pub fn next_frame(&mut self, timeout: Duration) -> Option<SseFrame> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = self.try_extract_frame() {
                return Some(frame);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match read_some(&mut self.stream, remaining.min(Duration::from_millis(200))) {
                Some(chunk) if !chunk.is_empty() => {
                    self.decoder.feed(&chunk);
                    self.text_buf.extend(self.decoder.take_decoded());
                }
                Some(_) => continue, // 超时但连接还在，接着等
                None => return None, // 连接关闭
            }
        }
    }

    /// `text_buf` 里有没有一个完整的「事件块」（以 `\n\n` 结尾）；有就解析出来
    /// 并从缓冲里移除，跳过纯注释块（只含 `:`开头的行）。
    fn try_extract_frame(&mut self) -> Option<SseFrame> {
        loop {
            let text = String::from_utf8_lossy(&self.text_buf);
            let pos = text.find("\n\n")?;
            let block = text[..pos].to_string();
            let consumed_bytes = pos + 2;
            self.text_buf.drain(..consumed_bytes);

            let mut id = None;
            let mut data = None;
            let mut only_comments = true;
            for line in block.split('\n') {
                let line = line.strip_suffix('\r').unwrap_or(line);
                if line.starts_with(':') {
                    continue; // SSE 注释（心跳），整块可能只有这一行
                }
                only_comments = false;
                if let Some(rest) = line.strip_prefix("id: ") {
                    id = rest.parse::<u64>().ok();
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    data = Some(rest.to_string());
                }
            }
            if only_comments {
                continue; // 纯心跳块，接着找下一个 \n\n 块
            }
            if let Some(data) = data {
                return Some(SseFrame { id, data });
            }
            // 有内容但没有 data 行——不是我们认识的帧形状，跳过继续找。
        }
    }
}
