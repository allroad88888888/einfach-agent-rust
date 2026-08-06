use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use serde_json::Value;
use tower::ServiceExt;
use tracing::instrument::WithSubscriber;
use tracing_subscriber::fmt::MakeWriter;

use super::observability;

#[tokio::test]
async fn request_log_uses_matched_route_and_excludes_raw_request_data() {
    let router = observability::instrument(Router::new().route(
        "/sessions/{id}/input",
        post(|| async { StatusCode::ACCEPTED }),
    ));
    let request = Request::builder()
        .method("POST")
        .uri("/sessions/private-session/input?capability=private-capability")
        .header("authorization", "Bearer private-token")
        .header("x-request-id", "client-controlled-request-id")
        .body(Body::from("private-message-body"))
        .unwrap();

    let (status, logs) = capture_request(router, request).await;

    assert_eq!(status, StatusCode::ACCEPTED);
    let completion = completion_event(&logs);
    assert_eq!(completion["span"]["name"], "http.request");
    assert_eq!(completion["span"]["method"], "POST");
    assert_eq!(completion["span"]["matched_route"], "/sessions/{id}/input");
    assert_eq!(completion["span"]["status"], 202);
    assert!(completion["span"]["elapsed_ms"].is_u64());

    let request_id = completion["span"]["request_id"].as_str().unwrap();
    assert!(request_id.starts_with("req-"));
    assert_ne!(request_id, "client-controlled-request-id");

    let rendered = render_logs(&logs);
    for secret in [
        "private-session",
        "private-capability",
        "private-token",
        "client-controlled-request-id",
        "private-message-body",
    ] {
        assert!(!rendered.contains(secret), "log leaked {secret}");
    }
}

#[tokio::test]
async fn unmatched_request_uses_a_fixed_safe_route_label() {
    let router =
        observability::instrument(Router::new().fallback(|| async { StatusCode::NOT_FOUND }));
    let request = Request::builder()
        .uri("/private/unmatched/path?token=private-query")
        .body(Body::empty())
        .unwrap();

    let (status, logs) = capture_request(router, request).await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    let completion = completion_event(&logs);
    assert_eq!(completion["span"]["matched_route"], "unmatched");
    let rendered = render_logs(&logs);
    assert!(!rendered.contains("private/unmatched"));
    assert!(!rendered.contains("private-query"));
}

async fn capture_request(router: Router, request: Request<Body>) -> (StatusCode, Vec<Value>) {
    let output = CapturedOutput::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_target(false)
        .with_writer(output.clone())
        .finish();

    let response = router
        .oneshot(request)
        .with_subscriber(subscriber)
        .await
        .unwrap();
    let logs = output
        .text()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    (response.status(), logs)
}

fn completion_event(logs: &[Value]) -> &Value {
    logs.iter()
        .find(|log| log["fields"]["message"] == "HTTP request completed")
        .expect("completion event")
}

fn render_logs(logs: &[Value]) -> String {
    serde_json::to_string(logs).unwrap()
}

#[derive(Clone, Default)]
struct CapturedOutput(Arc<Mutex<Vec<u8>>>);

impl CapturedOutput {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl<'writer> MakeWriter<'writer> for CapturedOutput {
    type Writer = CapturedWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CapturedWriter(self.0.clone())
    }
}

struct CapturedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
