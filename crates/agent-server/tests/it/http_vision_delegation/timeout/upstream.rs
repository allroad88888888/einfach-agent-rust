//! A controllable Kimi upload endpoint for the timeout E2E.
//!
//! The first upload request stays physically blocked until the test releases it. This lets the
//! HTTP/SSE assertions distinguish local call convergence from completion of blocking transport
//! work without claiming that the socket itself was aborted.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

pub struct GatedVisionUpstream {
    port: u16,
    gate: Arc<(Mutex<bool>, Condvar)>,
    uploads_started: Arc<AtomicUsize>,
    uploads_finished: Arc<AtomicUsize>,
    chats_started: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
}

impl GatedVisionUpstream {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind gated vision upstream");
        let port = listener.local_addr().expect("gated upstream addr").port();
        listener
            .set_nonblocking(true)
            .expect("nonblocking gated upstream");

        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let uploads_started = Arc::new(AtomicUsize::new(0));
        let uploads_finished = Arc::new(AtomicUsize::new(0));
        let chats_started = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let gate_bg = Arc::clone(&gate);
        let uploads_started_bg = Arc::clone(&uploads_started);
        let uploads_finished_bg = Arc::clone(&uploads_finished);
        let chats_started_bg = Arc::clone(&chats_started);
        let stop_bg = Arc::clone(&stop);

        thread::spawn(move || {
            loop {
                if stop_bg.load(Ordering::Acquire) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = stream.set_nonblocking(false);
                        let gate = Arc::clone(&gate_bg);
                        let uploads_started = Arc::clone(&uploads_started_bg);
                        let uploads_finished = Arc::clone(&uploads_finished_bg);
                        let chats_started = Arc::clone(&chats_started_bg);
                        thread::spawn(move || {
                            serve(
                                stream,
                                &gate,
                                &uploads_started,
                                &uploads_finished,
                                &chats_started,
                            )
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => return,
                }
            }
        });

        Self {
            port,
            gate,
            uploads_started,
            uploads_finished,
            chats_started,
            stop,
        }
    }

    pub fn chat_endpoint(&self) -> String {
        format!("http://127.0.0.1:{}/openai/v1/chat/completions", self.port)
    }

    pub fn upload_base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    pub fn uploads_started(&self) -> usize {
        self.uploads_started.load(Ordering::Acquire)
    }

    pub fn uploads_finished(&self) -> usize {
        self.uploads_finished.load(Ordering::Acquire)
    }

    pub fn chats_started(&self) -> usize {
        self.chats_started.load(Ordering::Acquire)
    }

    pub fn release_upload(&self) {
        open_gate(&self.gate);
    }
}

impl Drop for GatedVisionUpstream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        open_gate(&self.gate);
    }
}

fn serve(
    mut stream: TcpStream,
    gate: &(Mutex<bool>, Condvar),
    uploads_started: &AtomicUsize,
    uploads_finished: &AtomicUsize,
    chats_started: &AtomicUsize,
) {
    let Some(path) = read_path(&mut stream) else {
        return;
    };
    if path.ends_with("/files") {
        uploads_started.fetch_add(1, Ordering::AcqRel);
        wait_for_gate(gate);
        write_json(&mut stream, r#"{"id":"late-upload-reference"}"#);
        uploads_finished.fetch_add(1, Ordering::AcqRel);
    } else {
        chats_started.fetch_add(1, Ordering::AcqRel);
        write_chat(&mut stream);
    }
}

fn read_path(stream: &mut TcpStream) -> Option<String> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
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
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(path)
}

fn wait_for_gate(gate: &(Mutex<bool>, Condvar)) {
    let (opened, signal) = gate;
    let mut opened = opened.lock().expect("lock upload gate");
    while !*opened {
        opened = signal.wait(opened).expect("wait for upload release");
    }
}

fn open_gate(gate: &(Mutex<bool>, Condvar)) {
    let (opened, signal) = gate;
    *opened.lock().expect("open upload gate") = true;
    signal.notify_all();
}

fn write_json(stream: &mut TcpStream, body: &str) {
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn write_chat(stream: &mut TcpStream) {
    let body = "data: {\"choices\":[{\"delta\":{\"content\":\"late\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}
