//! 独立手写的 HTTP/1.1 chunked transfer-encoding 解码器（031 独测自己的实现，
//! 不看实现方 `tests/support/http_client.rs` 怎么写的）。axum 的 SSE 响应没有
//! `Content-Length`（长度未知的流），HTTP/1.1 下只能是 chunked：每个 chunk 是
//! `<hex 长度>\r\n<数据>\r\n`，长度 0 的 chunk 表示结束。
//!
//! 增量喂字节（`feed`），已解出的负载字节攒在内部缓冲，`take_decoded` 取走。
//! 不要求调用方按 chunk 边界喂数据——`process` 自己在不够一个完整 chunk 时
//! 停手等下一次 `feed`。

#![allow(dead_code)]

pub struct ChunkedDecoder {
    raw: Vec<u8>,
    decoded: Vec<u8>,
    finished: bool,
}

impl ChunkedDecoder {
    pub fn new() -> Self {
        ChunkedDecoder {
            raw: Vec::new(),
            decoded: Vec::new(),
            finished: false,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.raw.extend_from_slice(bytes);
        self.process();
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// 取走目前已经解出、还没被取走的负载字节。
    pub fn take_decoded(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.decoded)
    }

    fn process(&mut self) {
        loop {
            if self.finished {
                return;
            }
            let Some(line_end) = find_crlf(&self.raw) else {
                return;
            };
            let size_line = std::str::from_utf8(&self.raw[..line_end])
                .unwrap_or("")
                .trim();
            // chunk extension（`;` 之后）不出现在这个仓库的响应里，但按标准剥掉。
            let size_str = size_line.split(';').next().unwrap_or("0");
            let Ok(size) = usize::from_str_radix(size_str, 16) else {
                // 不是合法的 chunk-size 行——不是我们期望的协议，直接判结束避免死循环。
                self.finished = true;
                return;
            };

            let data_start = line_end + 2; // 跳过 chunk-size 行自己的 CRLF
            let data_end = data_start + size;
            let needed_total = data_end + 2; // 数据后面还有一个 CRLF
            if self.raw.len() < needed_total {
                return; // 这个 chunk 还没收全，等下一次 feed
            }

            if size == 0 {
                self.finished = true;
                self.raw.clear();
                return;
            }

            self.decoded
                .extend_from_slice(&self.raw[data_start..data_end]);
            self.raw.drain(..needed_total);
        }
    }
}

fn find_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_single_chunk_fed_whole() {
        let mut d = ChunkedDecoder::new();
        d.feed(b"5\r\nhello\r\n0\r\n\r\n");
        assert_eq!(d.take_decoded(), b"hello");
        assert!(d.is_finished());
    }

    #[test]
    fn decodes_a_chunk_split_across_multiple_feeds() {
        let mut d = ChunkedDecoder::new();
        d.feed(b"5\r\nhe");
        assert_eq!(d.take_decoded(), b"");
        d.feed(b"llo\r\n0");
        assert_eq!(d.take_decoded(), b"hello");
        d.feed(b"\r\n\r\n");
        assert!(d.is_finished());
    }

    #[test]
    fn decodes_multiple_chunks_in_one_feed() {
        let mut d = ChunkedDecoder::new();
        d.feed(b"2\r\nab\r\n3\r\ncde\r\n0\r\n\r\n");
        assert_eq!(d.take_decoded(), b"abcde");
        assert!(d.is_finished());
    }
}
