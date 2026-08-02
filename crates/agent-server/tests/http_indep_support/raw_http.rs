//! 非流式请求（`POST`/`GET` 除 `/events` 之外的六个端点）的一次性收发：连接、
//! 发请求、读完整个响应体、断开。独立手写，不看实现方的 `http_client.rs`。

#![allow(dead_code)]

use std::io::Write;
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use super::chunked::ChunkedDecoder;
use super::wire::{ResponseHead, read_head, read_some};

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body_str()).unwrap_or_else(|e| panic!("响应体不是合法 JSON：{e}，body={:?}", self.body_str()))
    }
}

const TIMEOUT: Duration = Duration::from_secs(5);

/// 发一个请求，等到响应体读完为止（`Content-Length` 精确读够；没有
/// `Content-Length` 时按 chunked 解到收尾 chunk；两者都没有就读到连接关闭）。
pub fn request(addr: SocketAddr, method: &str, path: &str, extra_headers: &[(&str, &str)], body: Option<&[u8]>) -> Response {
    let mut stream = TcpStream::connect(addr).unwrap_or_else(|e| panic!("connect {addr} 失败：{e}"));
    let body = body.unwrap_or(&[]);
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n");
    if !body.is_empty() {
        req.push_str("Content-Type: application/json\r\n");
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).expect("write request head");
    stream.write_all(body).expect("write request body");

    let (head, leftover) = read_head(&mut stream, TIMEOUT);
    let body = read_body(&mut stream, &head, leftover);
    Response { status: head.status, headers: head.headers, body }
}

pub fn get(addr: SocketAddr, path: &str) -> Response {
    request(addr, "GET", path, &[], None)
}

pub fn post_json(addr: SocketAddr, path: &str, json_body: &str) -> Response {
    request(addr, "POST", path, &[], Some(json_body.as_bytes()))
}

fn read_body(stream: &mut TcpStream, head: &ResponseHead, leftover: Vec<u8>) -> Vec<u8> {
    if let Some(cl) = head.header("content-length").and_then(|v| v.parse::<usize>().ok()) {
        let mut body = leftover;
        while body.len() < cl {
            match read_some(stream, TIMEOUT) {
                Some(chunk) if !chunk.is_empty() => body.extend_from_slice(&chunk),
                _ => break,
            }
        }
        body.truncate(cl);
        return body;
    }
    if head.header("transfer-encoding").map(|v| v.eq_ignore_ascii_case("chunked")).unwrap_or(false) {
        let mut decoder = ChunkedDecoder::new();
        decoder.feed(&leftover);
        while !decoder.is_finished() {
            match read_some(stream, TIMEOUT) {
                Some(chunk) if !chunk.is_empty() => decoder.feed(&chunk),
                _ => break,
            }
        }
        return decoder.take_decoded();
    }
    // 没有 Content-Length 也不是 chunked：读到连接关闭（比如某些 204/202 空体响应）。
    let mut body = leftover;
    loop {
        match read_some(stream, Duration::from_millis(300)) {
            Some(chunk) if !chunk.is_empty() => body.extend_from_slice(&chunk),
            _ => break,
        }
    }
    body
}
