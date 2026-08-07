//! `Client::post_json` 的非流式 JSON POST 接缝验收（s5 识图工具用）。
//!
//! 用本地 `TcpListener` 起假上游：断言请求行 / Authorization / Content-Type /
//! body 原样送达，然后按用例回不同的响应（200 JSON / 4xx / 拒连）。跟
//! `fake_sse.rs` / `file_upload_success.rs` 同一套「假上游」手法，只验证
//! transport 这一层，不接任何真实 provider。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use agent_transport::{Client, TransportError};

const API_KEY: &str = "sk-post-json-test-key";

/// 读完整请求：返回 (请求行, Authorization, Content-Type, body)。
fn read_request(stream: &mut TcpStream) -> (String, String, String, Vec<u8>) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request_line = String::new();
    reader.read_line(&mut request_line).unwrap();
    let mut headers = Vec::new();
    let mut content_length = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = Some(value.trim().parse::<usize>().unwrap());
        }
        headers.push(line.trim_end().to_owned());
    }
    let mut body = vec![0; content_length.unwrap_or(0)];
    reader.read_exact(&mut body).unwrap();
    let header = |name: &str| {
        headers
            .iter()
            .find_map(|line| {
                line.split_once(':')
                    .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.trim().to_owned())
            })
            .unwrap_or_default()
    };
    (
        request_line.trim_end().to_owned(),
        header("Authorization"),
        header("Content-Type"),
        body,
    )
}

fn write_response(stream: &mut TcpStream, status_line: &str, content_type: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

#[test]
fn post_json_sends_expected_request_and_returns_body_on_2xx() {
    let (listener, port) = listener();
    let payload = br#"{"model":"kimi-k3","messages":[]}"#.to_vec();
    let expected_payload = payload.clone();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        let (request_line, authorization, content_type, body) = read_request(&mut stream);
        assert_eq!(request_line, "POST /v1/chat/completions HTTP/1.1");
        assert_eq!(authorization, format!("Bearer {API_KEY}"));
        assert_eq!(content_type, "application/json");
        assert_eq!(body, expected_payload);
        write_response(
            &mut stream,
            "200 OK",
            "application/json",
            r#"{"choices":[{"message":{"content":"一只猫"}}]}"#,
        );
    });

    let (status, text) = Client::new()
        .post_json(
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            API_KEY,
            &payload,
        )
        .unwrap();

    server.join().unwrap();
    assert_eq!(status, 200);
    assert!(text.contains("一只猫"), "2xx 响应体应完整回传: {text}");
}

#[test]
fn post_json_maps_non_2xx_to_http_error_with_status() {
    let (listener, port) = listener();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        let _ = read_request(&mut stream);
        write_response(&mut stream, "429 Too Many Requests", "application/json", r#"{"error":"rate limited"}"#);
    });

    let error = Client::new()
        .post_json(
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            API_KEY,
            b"{}",
        )
        .unwrap_err();

    server.join().unwrap();
    match error {
        TransportError::Http { status, body } => {
            assert_eq!(status, 429);
            assert!(body.contains("rate limited"), "错误体应原样带回来: {body}");
        }
        other => panic!("非 2xx 应分类为 Http，实际 {other:?}"),
    }
}

#[test]
fn post_json_maps_refused_connection_to_connect_error() {
    // 拿一个端口再立刻放手，让这个地址处于「没人监听」状态——ureq 会立即拒连。
    let (listener, port) = listener();
    drop(listener);

    let error = Client::new()
        .post_json(
            &format!("http://127.0.0.1:{port}/v1/chat/completions"),
            API_KEY,
            b"{}",
        )
        .unwrap_err();

    assert!(
        matches!(error, TransportError::Connect { .. }),
        "拒连应分类为 Connect，实际 {error:?}"
    );
}
