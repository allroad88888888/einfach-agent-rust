//! 同时扮演 Kimi 聊天与上传端点的本地假服务。
//!
//! 聊天是 `/openai/v1/chat/completions`，上传是 `/v1/files`。它们只为简化
//! fixture 而共用 listener；路径刻意不同，以防聊天路径被错误地复用为上传基址。

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

#[derive(Clone)]
#[allow(dead_code)]
pub enum UploadReply {
    Ok,
    Status(u16),
    SlowOk(Duration),
}

#[derive(Clone)]
pub struct Call {
    pub path: String,
    #[allow(dead_code)] // 每个 integration test 单独编译；仅部分断言需要请求体。
    pub body: String,
}

pub struct ImageUploadUpstream {
    port: u16,
    calls: Arc<Mutex<Vec<Call>>>,
    stop: Arc<AtomicBool>,
}

impl ImageUploadUpstream {
    pub fn start(upload_reply: UploadReply) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind 图片假上游");
        let port = listener.local_addr().expect("图片假上游地址").port();
        listener
            .set_nonblocking(true)
            .expect("图片假上游设成非阻塞");

        let calls = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let calls_bg = Arc::clone(&calls);
        let stop_bg = Arc::clone(&stop);
        thread::spawn(move || {
            loop {
                if stop_bg.load(Ordering::Relaxed) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let calls = Arc::clone(&calls_bg);
                        let reply = upload_reply.clone();
                        thread::spawn(move || serve(stream, &calls, reply));
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(_) => return,
                }
            }
        });

        ImageUploadUpstream { port, calls, stop }
    }

    pub fn chat_endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/openai/v1/chat/completions", self.port)
    }

    pub fn upload_base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("读取图片假上游调用").clone()
    }

    #[allow(dead_code)] // 同上：并发测试只检查时序，线形测试才读取聊天体。
    pub fn chat_bodies(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| !call.path.ends_with("/files"))
            .map(|call| call.body)
            .collect()
    }

    #[allow(dead_code)]
    pub fn upload_count(&self) -> usize {
        self.calls()
            .into_iter()
            .filter(|call| call.path.ends_with("/files"))
            .count()
    }
}

impl Drop for ImageUploadUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn serve(mut stream: TcpStream, calls: &Mutex<Vec<Call>>, upload_reply: UploadReply) {
    let Some((path, body)) = read_request(&mut stream) else {
        return;
    };
    calls.lock().expect("记录图片假上游调用").push(Call {
        path: path.clone(),
        body,
    });
    if path.ends_with("/files") {
        match upload_reply {
            UploadReply::Ok => write_json(&mut stream, 200, r#"{"id":"uploaded-image"}"#),
            UploadReply::Status(status) => {
                write_json(&mut stream, status, r#"{"error":"rejected"}"#)
            }
            UploadReply::SlowOk(delay) => {
                thread::sleep(delay);
                write_json(&mut stream, 200, r#"{"id":"uploaded-image"}"#);
            }
        }
    } else {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).expect("写模型 SSE");
        stream.flush().expect("flush 模型 SSE");
    }
}

fn read_request(stream: &mut TcpStream) -> Option<(String, String)> {
    let mut reader = BufReader::new(stream.try_clone().expect("clone 图片假上游连接"));
    let mut first = String::new();
    if reader.read_line(&mut first).ok()? == 0 {
        return None;
    }
    let path = first.split_whitespace().nth(1)?.to_string();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if line == "\r\n" {
            break;
        }
        if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            content_length = value.trim().parse().ok()?;
        }
    }
    let mut bytes = vec![0; content_length];
    reader.read_exact(&mut bytes).ok()?;
    Some((path, String::from_utf8_lossy(&bytes).into_owned()))
}

fn write_json(stream: &mut TcpStream, status: u16, body: &str) {
    let response = format!(
        "HTTP/1.1 {status} test\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).expect("写上传响应");
    stream.flush().expect("flush 上传响应");
}
