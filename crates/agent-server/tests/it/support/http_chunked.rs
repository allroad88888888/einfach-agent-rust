//! HTTP/1.1 chunked transfer decoder shared by the raw test HTTP client and SSE reader.

pub(super) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[derive(Default)]
pub(super) struct ChunkDecoder {
    pub(super) raw: Vec<u8>,
    pub(super) decoded: Vec<u8>,
    pub(super) done: bool,
}

impl ChunkDecoder {
    pub(super) fn feed(&mut self, bytes: &[u8]) {
        self.raw.extend_from_slice(bytes);
        loop {
            let Some(line_end) = find(&self.raw, b"\r\n") else {
                return;
            };
            let size_str = String::from_utf8_lossy(&self.raw[..line_end]);
            let Ok(size) = usize::from_str_radix(size_str.trim(), 16) else {
                self.done = true;
                return;
            };
            let needed = line_end + 2 + size + 2;
            if self.raw.len() < needed {
                return;
            }
            if size == 0 {
                self.done = true;
                return;
            }
            self.decoded
                .extend_from_slice(&self.raw[line_end + 2..line_end + 2 + size]);
            self.raw.drain(..needed);
        }
    }
}
