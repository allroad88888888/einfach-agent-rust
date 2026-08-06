//! 安全的 HTTP 运维观测边界。
//!
//! 这里只产生结构化 span/event；可执行宿主负责安装全局 subscriber。字段刻意只取
//! HTTP method、Axum 模板路由、服务端生成的请求 ID、状态码与耗时，绝不读取 URI、
//! query、header 或 body。

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::extract::{MatchedPath, Request};
use axum::response::Response;
use tower_http::trace::TraceLayer;
use tracing::{Span, field};

const UNMATCHED_ROUTE: &str = "unmatched";
static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// 在所有 API 路由与可选静态 fallback 合成后包裹一次，保证所有 HTTP 请求都进入
/// 同一条观测管线。`Router::layer` 的位置也让 Axum 已经写入的 `MatchedPath` 可见。
pub(in crate::http) fn instrument(router: Router) -> Router {
    router.layer(
        TraceLayer::new_for_http()
            .make_span_with(make_request_span)
            .on_request(())
            .on_response(record_response)
            .on_body_chunk(())
            .on_eos(())
            .on_failure(()),
    )
}

fn make_request_span(request: &Request) -> Span {
    let request_id = next_request_id();
    let matched_route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or(UNMATCHED_ROUTE);

    tracing::info_span!(
        target: "agent_server::http",
        "http.request",
        request_id = %request_id,
        method = %request.method(),
        matched_route,
        status = field::Empty,
        elapsed_ms = field::Empty,
    )
}

fn record_response(response: &Response, latency: Duration, span: &Span) {
    let status = u64::from(response.status().as_u16());
    let elapsed_ms = latency.as_millis().min(u128::from(u64::MAX)) as u64;

    span.record("status", status);
    span.record("elapsed_ms", elapsed_ms);
    tracing::info!(
        target: "agent_server::http",
        parent: span,
        status,
        elapsed_ms,
        "HTTP request completed"
    );
}

/// PID + UNIX 纳秒 + 进程内序号保证请求 ID 由本服务生成，并在并发与进程重启后仍
/// 具有足够的区分度。它不会读取或回显任何客户端 request-id header。
fn next_request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("req-{:x}-{timestamp:x}-{sequence:x}", std::process::id())
}
