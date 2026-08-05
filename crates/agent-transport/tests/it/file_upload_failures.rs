//! 图片上传失败路径的 HTTP 接缝验收测试（issue 084）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use agent_transport::{Client, ImageUpload, MAX_IMAGE_BYTES, UploadError};

const API_KEY: &str = "sk-upload-test-key-must-not-appear-in-errors";

fn image(bytes: &[u8]) -> ImageUpload<'_> {
    ImageUpload {
        file_name: "pixel.png",
        mime_type: "image/png",
        bytes,
    }
}

fn drain_request(stream: &mut TcpStream) {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().unwrap();
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).unwrap();
}

fn write_response(stream: &mut TcpStream, status: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

fn assert_key_redacted(error: &UploadError) {
    assert!(
        !error.to_string().contains(API_KEY),
        "Display 输出不得泄露 API key"
    );
    assert!(
        !format!("{error:?}").contains(API_KEY),
        "Debug 输出不得泄露 API key"
    );
}

#[test]
fn oversize_image_is_rejected_before_any_http_request() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    listener.set_nonblocking(true).unwrap();
    let requests = Arc::new(AtomicUsize::new(0));
    let request_count = Arc::clone(&requests);
    let server = thread::spawn(move || {
        // 突变时 multipart 会复制超过 100 MiB；留足窗口才能观察到那次本不该发生的
        // 连接，而不是让 server 先退出并把它误报成网络错误。
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream.set_nonblocking(false).unwrap();
                    request_count.fetch_add(1, Ordering::Relaxed);
                    drain_request(&mut stream);
                    write_response(&mut stream, 200, r#"{"id":"unexpected"}"#);
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(error) => panic!("accept 假 server 失败: {error}"),
            }
        }
    });

    let oversized = vec![0; MAX_IMAGE_BYTES + 1];
    let result = Client::new().upload_image(
        &format!("http://127.0.0.1:{port}/v1"),
        API_KEY,
        image(&oversized),
    );
    server.join().unwrap();
    assert_eq!(
        requests.load(Ordering::Relaxed),
        0,
        "超过大小限制时不得连到假 server"
    );
    let error = result.expect_err("超过 100 MiB 必须在发请求前被拦截");
    assert!(
        matches!(error, UploadError::TooLarge { actual_bytes, limit_bytes } if actual_bytes == MAX_IMAGE_BYTES + 1 && limit_bytes == MAX_IMAGE_BYTES)
    );
    assert_key_redacted(&error);
}

#[test]
fn distinguishes_http_rejections_without_leaking_the_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        for status in [401, 413, 500] {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_nonblocking(false).unwrap();
            drain_request(&mut stream);
            write_response(
                &mut stream,
                status,
                &format!(r#"{{"error":"reflected {API_KEY}"}}"#),
            );
        }
    });
    let base_url = format!("http://127.0.0.1:{port}/v1");
    let client = Client::new();

    let unauthorized = client
        .upload_image(&base_url, API_KEY, image(b"401"))
        .unwrap_err();
    assert!(matches!(unauthorized, UploadError::Unauthorized));
    assert_key_redacted(&unauthorized);
    let too_large = client
        .upload_image(&base_url, API_KEY, image(b"413"))
        .unwrap_err();
    assert!(matches!(
        too_large,
        UploadError::ProviderRejected { status: 413 }
    ));
    assert_key_redacted(&too_large);
    let rejected = client
        .upload_image(&base_url, API_KEY, image(b"500"))
        .unwrap_err();
    assert!(matches!(
        rejected,
        UploadError::ProviderRejected { status: 500 }
    ));
    assert_key_redacted(&rejected);
    server.join().unwrap();
}

#[test]
fn malformed_or_unreachable_responses_also_redact_the_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        drain_request(&mut stream);
        write_response(&mut stream, 200, &format!(r#"{{"message":"{API_KEY}"}}"#));
    });

    let malformed = Client::new()
        .upload_image(
            &format!("http://127.0.0.1:{port}/v1"),
            API_KEY,
            image(b"bad response"),
        )
        .unwrap_err();
    assert!(matches!(malformed, UploadError::InvalidResponse { .. }));
    assert_key_redacted(&malformed);
    server.join().unwrap();

    let probe = TcpListener::bind("127.0.0.1:0").unwrap();
    let unused_port = probe.local_addr().unwrap().port();
    drop(probe);
    let network = Client::new()
        .upload_image(
            &format!("http://127.0.0.1:{unused_port}/v1"),
            API_KEY,
            image(b"network"),
        )
        .unwrap_err();
    assert!(matches!(network, UploadError::Network { .. }));
    assert_key_redacted(&network);
}
