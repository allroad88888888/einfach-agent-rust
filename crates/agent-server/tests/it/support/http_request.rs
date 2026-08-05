//! 假浏览器的普通 HTTP 请求收发。SSE 增量读取留在 `http_client`，这里仅负责
//! 完整响应，避免测试传输辅助器把两类协议堆进同一个大文件。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use super::http_chunked::ChunkDecoder;
use super::http_response::HttpResponse;

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
    let stream = TcpStream::connect(addr).expect("连接假浏览器目标地址");
    stream
        .set_read_timeout(Some(Duration::from_millis(50)))
        .expect("设置短读超时,给增量轮询用");
    let mut stream = stream;

    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n");
    for (k, v) in extra_headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    if let Some(b) = body {
        head.push_str("Content-Type: application/json\r\n");
        head.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).expect("写请求头");
    if let Some(b) = body {
        stream.write_all(b.as_bytes()).expect("写请求体");
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
    if let Some(length) =
        header(headers, "content-length").and_then(|value| value.parse::<usize>().ok())
    {
        let mut buffer = vec![0u8; length];
        reader.read_exact(&mut buffer).unwrap_or(());
        return String::from_utf8_lossy(&buffer).into_owned();
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
        return String::from_utf8_lossy(&decoder.decoded).into_owned();
    }
    let mut buffer = Vec::new();
    let _ = reader.read_to_end(&mut buffer);
    String::from_utf8_lossy(&buffer).into_owned()
}
