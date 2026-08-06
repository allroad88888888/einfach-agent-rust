use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use agent_core::{ContentBlock, Message, MessageId, Role, SessionConfig};
use agent_providers::kimi::Kimi;
use agent_transport::{Backoff, Client};

use super::{OwnedIngredients, ProviderRequest};
use crate::image_preparation_failure::ImagePreparationFailure;
use crate::{ExecutionBinding, ImageResolveError, ImageResolver, ResolvedImageLease};

struct TestLease {
    drops: Arc<AtomicUsize>,
    bytes: Vec<u8>,
}

impl Drop for TestLease {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::Relaxed);
    }
}

impl ResolvedImageLease for TestLease {
    fn mime(&self) -> &str {
        "image/png"
    }

    fn name(&self) -> Option<&str> {
        Some("private-name.png")
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

struct TestResolver {
    drops: Arc<AtomicUsize>,
    leases: AtomicUsize,
}

impl ImageResolver for TestResolver {
    fn lease(&self, handle: &str) -> Result<Box<dyn ResolvedImageLease>, ImageResolveError> {
        assert!(matches!(handle, "first" | "second"));
        self.leases.fetch_add(1, Ordering::Relaxed);
        Ok(Box::new(TestLease {
            drops: Arc::clone(&self.drops),
            bytes: b"private-image-body".to_vec(),
        }))
    }
}

#[test]
fn pre_cancel_avoids_resolving_any_attachment() {
    let drops = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(TestResolver {
        drops: Arc::clone(&drops),
        leases: AtomicUsize::new(0),
    });
    let request = deferred_request(resolver.clone());
    let cancel = AtomicBool::new(true);

    let error = request.prepare(&binding("http://127.0.0.1:1/v1"), &cancel);

    assert!(matches!(error, Err(ImagePreparationFailure::Cancelled)));
    assert_eq!(resolver.leases.load(Ordering::Relaxed), 0);
    assert_eq!(drops.load(Ordering::Relaxed), 0);
}

#[test]
fn in_flight_cancel_lets_started_upload_finish_then_stops_the_batch() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(AtomicUsize::new(0));
    let server_requests = Arc::clone(&requests);
    let (seen_tx, seen_rx) = mpsc::sync_channel(0);
    let server = thread::spawn(move || serve_one_slow_upload(listener, server_requests, seen_tx));

    let drops = Arc::new(AtomicUsize::new(0));
    let resolver = Arc::new(TestResolver {
        drops: Arc::clone(&drops),
        leases: AtomicUsize::new(0),
    });
    let request = deferred_request(resolver.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let (cancelled_at_tx, cancelled_at_rx) = mpsc::channel();
    let canceller = thread::spawn(move || {
        seen_rx.recv().unwrap();
        let at = Instant::now();
        worker_cancel.store(true, Ordering::Relaxed);
        cancelled_at_tx.send(at).unwrap();
    });

    let error = request.prepare(&binding(&format!("http://127.0.0.1:{port}/v1")), &cancel);
    let cancelled_at = cancelled_at_rx.recv().unwrap();

    assert!(matches!(error, Err(ImagePreparationFailure::Cancelled)));
    assert!(
        cancelled_at.elapsed() >= Duration::from_millis(500),
        "the existing synchronous upload should finish before cancellation is observed: {:?}",
        cancelled_at.elapsed()
    );
    assert_eq!(resolver.leases.load(Ordering::Relaxed), 2);
    assert_eq!(
        drops.load(Ordering::Relaxed),
        2,
        "all leases are released after the started upload returns"
    );
    canceller.join().unwrap();
    server.join().unwrap();
    assert_eq!(
        requests.load(Ordering::Relaxed),
        1,
        "second upload must not start"
    );
}

fn deferred_request(resolver: Arc<dyn ImageResolver>) -> ProviderRequest {
    ProviderRequest::Deferred {
        ingredients: OwnedIngredients {
            system: Vec::new(),
            messages: vec![Message {
                id: MessageId(1),
                role: Role::User,
                blocks: ["first", "second"]
                    .into_iter()
                    .map(|handle| ContentBlock::Image {
                        reference: Arc::from(format!("attachment://{handle}")),
                        mime: Arc::from("image/png"),
                        name: None,
                    })
                    .collect(),
            }],
            tools: Vec::new(),
            late_tools: Vec::new(),
            late_system: Vec::new(),
            prev_prefix: None,
        },
        resolver,
    }
}

fn binding(upload_base: &str) -> ExecutionBinding {
    let client = Client::with_config(
        Duration::from_secs(1),
        Duration::from_millis(20),
        Backoff {
            base: Duration::ZERO,
            max_attempts: 1,
        },
    );
    ExecutionBinding::new(
        Arc::new(Kimi),
        Arc::new(client),
        format!("{upload_base}/chat/completions"),
        "sk-runtime-image-secret".to_owned(),
        SessionConfig {
            model: Arc::from("kimi-test"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
    )
    .with_image_upload_base_url(upload_base.to_owned())
}

fn serve_one_slow_upload(
    listener: TcpListener,
    requests: Arc<AtomicUsize>,
    seen: mpsc::SyncSender<()>,
) {
    let (mut stream, _) = listener.accept().unwrap();
    requests.fetch_add(1, Ordering::Relaxed);
    drain_request(&mut stream);
    seen.send(()).unwrap();
    thread::sleep(Duration::from_millis(700));
    let body = r#"{"id":"too-late"}"#;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());

    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_millis(100);
    while Instant::now() < deadline {
        match listener.accept() {
            Ok((_, _)) => {
                requests.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept failed: {error}"),
        }
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
