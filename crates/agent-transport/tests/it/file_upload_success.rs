//! 图片上传成功路径的 HTTP 接缝验收测试（issue 084）。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;

use agent_transport::{Client, ImageUpload};

const API_KEY: &str = "sk-upload-test-key-must-not-appear-in-errors";

struct Request {
    request_line: String,
    headers: Vec<String>,
    body: Vec<u8>,
}

struct MultipartPart {
    headers: String,
    body: Vec<u8>,
}

fn image(bytes: &[u8]) -> ImageUpload<'_> {
    ImageUpload {
        file_name: "pixel.png",
        mime_type: "image/png",
        bytes,
    }
}

fn read_request(stream: &mut TcpStream) -> Request {
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
    let mut body = vec![0; content_length.expect("上传请求必须带 Content-Length")];
    reader.read_exact(&mut body).unwrap();
    Request {
        request_line: request_line.trim_end().to_owned(),
        headers,
        body,
    }
}

fn header<'a>(request: &'a Request, name: &str) -> &'a str {
    request
        .headers
        .iter()
        .find_map(|line| {
            line.split_once(':')
                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value.trim())
        })
        .expect("请求缺少预期 header")
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
        .expect("格式错误的 multipart body")
}

fn multipart_parts(request: &Request) -> Vec<MultipartPart> {
    let content_type = header(request, "Content-Type");
    let boundary = content_type
        .split(';')
        .find_map(|item| item.trim().strip_prefix("boundary="))
        .expect("multipart Content-Type 必须带 boundary")
        .trim_matches('"');
    let marker = format!("--{boundary}").into_bytes();
    let mut rest = request.body.as_slice();
    assert!(
        rest.starts_with(&marker),
        "multipart body 必须从 boundary 开始"
    );
    rest = &rest[marker.len()..];
    assert!(rest.starts_with(b"\r\n"), "首个 boundary 后必须是 CRLF");
    rest = &rest[2..];
    let next_boundary = [b"\r\n".as_slice(), marker.as_slice()].concat();
    let mut parts = Vec::new();
    loop {
        let header_end = find_bytes(rest, b"\r\n\r\n");
        let headers =
            String::from_utf8(rest[..header_end].to_vec()).expect("multipart headers 必须是文本");
        rest = &rest[header_end + 4..];
        let body_end = find_bytes(rest, &next_boundary);
        parts.push(MultipartPart {
            headers,
            body: rest[..body_end].to_vec(),
        });
        rest = &rest[body_end + next_boundary.len()..];
        if rest.starts_with(b"--") {
            return parts;
        }
        assert!(
            rest.starts_with(b"\r\n"),
            "part boundary 后必须是 CRLF 或结束标志"
        );
        rest = &rest[2..];
    }
}

fn part<'a>(parts: &'a [MultipartPart], name: &str) -> &'a MultipartPart {
    let name = format!("name=\"{name}\"");
    parts
        .iter()
        .find(|part| part.headers.contains(&name))
        .expect("缺少 multipart 字段")
}

fn write_response(stream: &mut TcpStream) {
    let body = r#"{"id":"file-abc123"}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

#[test]
fn uploads_expected_multipart_and_returns_complete_ms_reference() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let sent_bytes = b"\x89PNG\r\n\x1a\n\0\xffraw-image";
    let expected_bytes = sent_bytes.to_vec();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // macOS/BSD 会把 listener 的 O_NONBLOCK 继承给 accepted socket。
        stream.set_nonblocking(false).unwrap();
        let request = read_request(&mut stream);
        assert_eq!(request.request_line, "POST /v1/files HTTP/1.1");
        assert!(header(&request, "Content-Type").starts_with("multipart/form-data;"));
        let parts = multipart_parts(&request);
        assert_eq!(part(&parts, "purpose").body, b"image");
        let file = part(&parts, "file");
        assert!(file.headers.contains("filename=\"pixel.png\""));
        assert!(file.headers.contains("Content-Type: image/png"));
        assert_eq!(file.body, expected_bytes);
        write_response(&mut stream);
    });

    let reference = Client::new()
        .upload_image(
            &format!("http://127.0.0.1:{port}/v1"),
            API_KEY,
            image(sent_bytes),
        )
        .unwrap();

    server.join().unwrap();
    assert_eq!(reference, "ms://file-abc123");
}
