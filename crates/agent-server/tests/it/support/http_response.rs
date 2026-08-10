//! The decoded response envelope returned by the raw HTTP test client.

/// A complete small HTTP response: status, headers, and decoded body.
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// 字节级响应 envelope：`body` 保留原始字节（不做 UTF-8 lossy 解码），给
/// `/uploads` 这种返回二进制（图片字节）的端点用。
pub struct HttpResponseBytes {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpResponseBytes {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}
