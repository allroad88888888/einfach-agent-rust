use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

const KIMI_VISION_MODEL: &str = "kimi-k3";

fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "agent-tools-vision-{name}-{}-{n}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn runtime(link_source: VisionLinkSource) -> VisionRuntime {
    VisionRuntime::new(
        Arc::new(Client::new()),
        "https://api.moonshot.cn/v1",
        "sk-vision-test-key",
        KIMI_VISION_MODEL,
        link_source,
    )
}

#[test]
fn spec_declares_image_and_question() {
    let spec = vision_inspect_spec();
    assert_eq!(&*spec.name, VISION_INSPECT_TOOL);
    let schema = &spec.schema;
    assert_eq!(schema["required"], json!(["image"]));
    assert_eq!(schema["properties"]["image"]["type"], json!("string"));
    assert_eq!(schema["properties"]["question"]["type"], json!("string"));
    assert!(schema["properties"]["question"]["description"].is_string());
}

#[test]
fn missing_image_is_bad_input() {
    let vision = runtime(VisionLinkSource::LocalRoot(temp_dir("missing")));
    let err = inspect(Some(&vision), &json!({ "question": "有什么？" })).unwrap_err();
    assert_eq!(&*err.code, "bad_input");
}

#[test]
fn not_configured_without_runtime() {
    let err = inspect(None, &json!({ "image": "/uploads/up-1" })).unwrap_err();
    assert_eq!(&*err.code, "not_configured");
}

#[test]
fn public_url_is_rejected_in_upload_dir_mode() {
    let vision = runtime(VisionLinkSource::UploadDir(temp_dir("public-url")));
    let err = inspect(
        Some(&vision),
        &json!({ "image": "https://example.com/cat.png" }),
    )
    .unwrap_err();
    assert_eq!(&*err.code, "bad_input");
}

#[test]
fn missing_uploaded_file_is_not_found() {
    let dir = temp_dir("missing-upload");
    let vision = runtime(VisionLinkSource::UploadDir(dir));
    let err = inspect(Some(&vision), &json!({ "image": "/uploads/up-999" })).unwrap_err();
    assert_eq!(&*err.code, "not_found");
}

#[test]
fn local_root_resolves_relative_path_within_root() {
    let root = temp_dir("local-root");
    std::fs::write(root.join("cat.png"), b"\x89PNG-fake-bytes").unwrap();
    let vision = runtime(VisionLinkSource::LocalRoot(root));
    // 本地 root 形态：先走到 fake Kimi（这里只验证解析阶段成功取到字节并
    // 开始上传——用不可能连通的 base_url 验证错误发生在 upload 阶段而不是
    // 解析阶段）。
    let mut vision = vision;
    vision.kimi_base_url = "http://127.0.0.1:1/v1".to_string();
    let err = inspect(Some(&vision), &json!({ "image": "cat.png" })).unwrap_err();
    assert_eq!(
        &*err.code,
        "upload_failed",
        "字节解析应成功、失败应发生在上传阶段：{}",
        err.message
    );
}

#[test]
fn local_root_rejects_dotdot_escape() {
    let root = temp_dir("local-root-escape");
    let vision = runtime(VisionLinkSource::LocalRoot(root));
    let err = inspect(Some(&vision), &json!({ "image": "../secret.png" })).unwrap_err();
    assert_eq!(&*err.code, "outside_root");
}

// ── 端到端：假 Kimi 上游（files 上传 → chat completions）────────────────

fn read_request(stream: &mut TcpStream) -> (String, Vec<u8>) {
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
    (request_line.trim_end().to_owned(), body)
}

fn write_response(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}

#[test]
fn end_to_end_uploads_bytes_then_chats_with_ms_reference() {
    let dir = temp_dir("e2e");
    std::fs::write(dir.join("up-7"), b"\x89PNG-raw-pixels").unwrap();
    std::fs::write(dir.join("up-7.mime"), "image/png").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        // 1) files 上传
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        let (request_line, body) = read_request(&mut stream);
        assert_eq!(request_line, "POST /v1/files HTTP/1.1");
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("name=\"purpose\""), "multipart 缺 purpose 字段");
        assert!(text.contains("name=\"file\""), "multipart 缺 file 字段");
        assert!(text.contains("image/png"), "multipart 缺 mime");
        assert!(body.windows(12).any(|w| w == b"\x89PNG-raw-pix"), "multipart 必须带原始字节");
        write_response(&mut stream, r#"{"id":"file-e2e-1"}"#);

        // 2) chat completions
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        let (request_line, body) = read_request(&mut stream);
        assert_eq!(request_line, "POST /v1/chat/completions HTTP/1.1");
        let text: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(text["model"], json!("kimi-k3"));
        let content = &text["messages"][0]["content"];
        assert_eq!(content[0]["type"], json!("image_url"));
        assert_eq!(content[0]["image_url"]["url"], json!("ms://file-e2e-1"));
        assert_eq!(content[1]["type"], json!("text"));
        assert_eq!(content[1]["text"], json!("这是什么动物？"));
        write_response(
            &mut stream,
            r#"{"choices":[{"message":{"content":"一只橘猫"}}]}"#,
        );
    });

    let mut vision = runtime(VisionLinkSource::UploadDir(dir));
    vision.kimi_base_url = format!("http://127.0.0.1:{port}/v1");
    let result = inspect(
        Some(&vision),
        &json!({ "image": "/uploads/up-7", "question": "这是什么动物？" }),
    )
    .unwrap();

    server.join().unwrap();
    assert_eq!(result, "一只橘猫");
}
