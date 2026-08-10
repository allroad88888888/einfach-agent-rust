//! 假浏览器的普通 HTTP 请求收发。SSE 增量读取留在 `http_client`，这里仅负责
//! 完整响应，避免测试传输辅助器把两类协议堆进同一个大文件。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::http_chunked::ChunkDecoder;
use super::http_response::{HttpResponse, HttpResponseBytes};

pub fn request(addr: SocketAddr, method: &str, path: &str, body: Option<&str>) -> HttpResponse {
    request_with_headers(addr, method, path, &[], body)
}

pub fn request_with_headers(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> HttpResponse {
    let headers = with_private_capability(headers);
    request_exact_headers(addr, method, path, &headers, body)
}

/// 不追加测试 capability 的原始请求。仅供私有 API 鉴权的负向矩阵使用，避免
/// 「测试辅助器总会帮忙带认证」掩盖服务端退化。
pub fn request_exact_headers(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> HttpResponse {
    let mut reader = connect_and_send(addr, method, path, headers, body);
    let (status, headers) = read_head(&mut reader);
    let body = read_full_body(&mut reader, &headers);
    HttpResponse {
        status,
        headers,
        body,
    }
}

/// 字节级 HTTP 往返：发任意原始 body（multipart 二进制），返回原始字节响应。
/// 自动追加测试 capability（同 [`request_with_headers`]）；Content-Type 必须由
/// 调用方在 `headers` 里给出——multipart 的 boundary 就在 Content-Type 里。
/// `/uploads` 集成测试用这个：图片字节含 `\x89PNG` 这类非 UTF-8 魔数，`&str`
/// 表达不了，必须走字节路径。
pub fn request_bytes_with_headers(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponseBytes {
    let headers = with_private_capability(headers);
    let mut reader = connect_and_send_bytes(addr, method, path, &headers, Some(body));
    let (status, headers) = read_head(&mut reader);
    let body = read_full_body_bytes(&mut reader, &headers);
    HttpResponseBytes {
        status,
        headers,
        body,
    }
}

pub(crate) fn with_private_capability<'a>(
    headers: &[(&'a str, &'a str)],
) -> Vec<(&'a str, &'a str)> {
    if headers
        .iter()
        .any(|(name, _)| name.eq_ignore_ascii_case("x-agent-server-capability"))
    {
        return headers.to_vec();
    }
    let mut result = Vec::with_capacity(headers.len() + 1);
    result.push((
        "x-agent-server-capability",
        super::http_server::PRIVATE_CAPABILITY,
    ));
    result.extend_from_slice(headers);
    result
}

pub(crate) fn connect_and_send(
    addr: SocketAddr,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&str>,
) -> BufReader<TcpStream> {
    send_raw(
        addr,
        method,
        path,
        extra_headers,
        Some("application/json"),
        body.map(str::as_bytes),
    )
}

/// 字节级版本：body 是任意原始字节（multipart 二进制），Content-Type 由调用方在
/// `extra_headers` 里给出（`send_raw` 不自动补）。
pub(crate) fn connect_and_send_bytes(
    addr: SocketAddr,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    body: Option<&[u8]>,
) -> BufReader<TcpStream> {
    send_raw(addr, method, path, extra_headers, None, body)
}

fn send_raw(
    addr: SocketAddr,
    method: &str,
    path: &str,
    extra_headers: &[(&str, &str)],
    content_type: Option<&str>,
    body: Option<&[u8]>,
) -> BufReader<TcpStream> {
    let stream = TcpStream::connect(addr).expect("连接假浏览器目标地址");
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("设置短读超时,给增量轮询用");
    let mut stream = stream;

    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(ct) = content_type {
        head.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    if let Some(b) = body {
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("写请求头");
    if let Some(b) = body {
        stream.write_all(b).expect("写请求体");
    }
    stream.flush().expect("flush 请求");
    BufReader::new(stream)
}

pub(crate) fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

pub(crate) fn read_head(reader: &mut BufReader<TcpStream>) -> (u16, Vec<(String, String)>) {
    let status_line = read_line_retrying(reader);
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut headers = Vec::new();
    loop {
        let line = read_line_retrying(reader);
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_string(), value.trim().to_string()));
        }
    }
    (status, headers)
}

fn read_line_retrying(reader: &mut BufReader<TcpStream>) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => return String::new(),
            Ok(_) => return line.trim_end_matches(['\r', '\n']).to_string(),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                if Instant::now() >= deadline {
                    panic!("读响应头超时");
                }
            }
            Err(error) => panic!("读响应头失败：{error}"),
        }
    }
}

fn read_full_body(reader: &mut BufReader<TcpStream>, headers: &[(String, String)]) -> String {
    String::from_utf8_lossy(&read_full_body_bytes(reader, headers)).into_owned()
}

/// 字节级读完整响应体，保留原始字节（不做 UTF-8 lossy 解码）——`GET /uploads/
/// {id}` 取回的是图片字节，lossy 解码会破坏二进制内容。
fn read_full_body_bytes(
    reader: &mut BufReader<TcpStream>,
    headers: &[(String, String)],
) -> Vec<u8> {
    if let Some(length) =
        header(headers, "content-length").and_then(|value| value.parse::<usize>().ok())
    {
        let mut buffer = vec![0u8; length];
        reader.read_exact(&mut buffer).unwrap_or(());
        return buffer;
    }
    if header(headers, "transfer-encoding")
        .map(|value| value.eq_ignore_ascii_case("chunked"))
        .unwrap_or(false)
    {
        let mut decoder = ChunkDecoder::default();
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    decoder.feed(&buffer[..read]);
                    if decoder.done {
                        break;
                    }
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => break,
            }
        }
        return decoder.decoded;
    }
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    buffer
}
