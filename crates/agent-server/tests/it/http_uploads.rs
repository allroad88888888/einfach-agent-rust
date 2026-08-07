//! `/uploads` 上传端点的集成测试（s5）。用原生 TcpStream 发 multipart——图片
//! 字节含非 UTF-8 魔数（`\x89PNG…`），`&str` 表达不了，走
//! `http_client::request_bytes_with_headers` 的字节级路径（见 `support/http_
//! request.rs`）。
//!
//! 覆盖：未配 `upload_dir` 时两个路由都不存在（404）；配了之后真实 PNG 上传 →
//! 200 + url → GET 取回逐字节一致；白名单外 mime 被拒（400）；声明 mime 与魔数
//! 不符被拒（400）；超过 100 MiB 上限被拒（≥400）。

use agent_transport::MAX_IMAGE_BYTES;
use crate::support::http_client::{
    request, request_bytes_with_headers, HttpResponseBytes,
};
use crate::support::http_server::{session_template, start_at_with_template};

const BOUNDARY: &str = "----agent-server-it-uploads";

/// 1×1 RGBA PNG（67 字节，IHDR/IDAT/IEND + CRC 齐全），真实图片魔数
/// `\x89PNG\r\n\x1a\n`。base64 `iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAf
/// FcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==` 解出。
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, // signature
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, // IHDR len 13
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, // 1×1
    0x08, 0x06, 0x00, 0x00, 0x00, // RGBA, 无压缩/滤波/隔行
    0x1f, 0x15, 0xc4, 0x89, // IHDR CRC
    0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, // IDAT len 13
    0x78, 0xda, 0x63, 0xfc, 0xcf, 0xc0, 0x50, 0x0f, 0x00, 0x04, 0x85, 0x01, 0x80, // zlib
    0x84, 0xa9, 0x8c, 0x21, // IDAT CRC
    0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, // IEND len 0
    0xae, 0x42, 0x60, 0x82, // IEND CRC
];

/// 拼一个 multipart/form-data 请求体：一个名为 `file` 的字段，`content_type`
/// 是字段自己的 Content-Type（服务端靠它进白名单）。
fn multipart(content_type: &str, file_bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"x.bin\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(file_bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn content_type_header() -> String {
    format!("multipart/form-data; boundary={BOUNDARY}")
}

/// 配了 `upload_dir` 的测试服务器（默认模板是 `upload_dir: None`，即「未启用」）。
async fn server_with_upload_dir() -> crate::support::http_server::TestServer {
    let mut template = session_template("http://upstream.test".to_string());
    template.upload_dir = Some(crate::support::temp_dir("http-uploads"));
    start_at_with_template("127.0.0.1:0".parse().unwrap(), template, |c| c).await
}

#[tokio::test(flavor = "multi_thread")]
async fn without_upload_dir_both_upload_routes_are_404() {
    let upstream = crate::support::server::FakeServer::start(vec![]);
    let server = crate::support::http_server::start(upstream.endpoint()).await;

    let body = multipart("image/png", PNG_1X1);
    let post: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "POST",
        "/uploads",
        &[("content-type", &content_type_header())],
        &body,
    );
    assert_eq!(post.status, 404, "未配 upload_dir 该没有 POST /uploads 路由");

    let get = request(server.addr, "GET", "/uploads/up-1-0", None);
    assert_eq!(get.status, 404, "未配 upload_dir 该没有 GET /uploads/{{id}} 路由");
}

#[tokio::test(flavor = "multi_thread")]
async fn upload_real_png_returns_url_and_get_round_trips_exact_bytes() {
    let server = server_with_upload_dir().await;

    let body = multipart("image/png", PNG_1X1);
    let post: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "POST",
        "/uploads",
        &[("content-type", &content_type_header())],
        &body,
    );
    assert_eq!(post.status, 200, "{}", String::from_utf8_lossy(&post.body));
    assert_eq!(
        post.header("content-type"),
        Some("application/json"),
        "{:?}",
        post.headers
    );
    let url = String::from_utf8(post.body).expect("200 响应该是 UTF-8 JSON");
    assert!(url.starts_with(r#"{"url":"/uploads/"#), "{url}");
    let id = url
        .trim()
        .strip_prefix(r#"{"url":"/uploads/"#)
        .and_then(|rest| rest.strip_suffix("\"}"))
        .expect("url 字段形状该是 /uploads/<id>");
    assert!(!id.is_empty() && id.starts_with("up-"), "id 该是 up- 前缀：{id}");

    // GET 取回：原始字节逐字节一致 + 上传时记录的 Content-Type。
    let get: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "GET",
        &format!("/uploads/{id}"),
        &[],
        &[],
    );
    assert_eq!(get.status, 200, "{}", String::from_utf8_lossy(&get.body));
    assert_eq!(get.body, PNG_1X1, "GET 该取回与上传逐字节一致的图片");
    assert_eq!(
        get.header("content-type"),
        Some("image/png"),
        "{:?}",
        get.headers
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn non_whitelisted_mime_rejected() {
    let server = server_with_upload_dir().await;

    let body = multipart("application/pdf", b"%PDF-1.4 fake");
    let post: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "POST",
        "/uploads",
        &[("content-type", &content_type_header())],
        &body,
    );
    assert_eq!(post.status, 400, "{}", String::from_utf8_lossy(&post.body));
    assert!(
        String::from_utf8_lossy(&post.body).contains("不支持的图片类型"),
        "{}",
        String::from_utf8_lossy(&post.body)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn mime_magic_mismatch_rejected() {
    let server = server_with_upload_dir().await;

    // 声明 image/png，内容却是纯文本——魔数校验该拦下来（s1 新增）。
    let body = multipart("image/png", b"definitely not a png");
    let post: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "POST",
        "/uploads",
        &[("content-type", &content_type_header())],
        &body,
    );
    assert_eq!(post.status, 400, "{}", String::from_utf8_lossy(&post.body));
    assert!(
        String::from_utf8_lossy(&post.body).contains("魔数"),
        "{}",
        String::from_utf8_lossy(&post.body)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_image_rejected() {
    let server = server_with_upload_dir().await;

    // file 内容顶到 100 MiB，multipart 边界/头开销之后整个请求体必然超
    // `MAX_IMAGE_BYTES`——axum DefaultBodyLimit（路由层 100 MiB）或 handler 的
    // 大小检查至少有一个触发，总之不能 2xx。
    let huge = vec![0u8; MAX_IMAGE_BYTES];
    let body = multipart("image/png", &huge);
    let post: HttpResponseBytes = request_bytes_with_headers(
        server.addr,
        "POST",
        "/uploads",
        &[("content-type", &content_type_header())],
        &body,
    );
    assert!(
        post.status >= 400 && post.status != 404,
        "超限该被拒，实际 {}",
        post.status
    );
}
