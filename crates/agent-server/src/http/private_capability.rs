//! 私有 session API 的进程级 capability 校验；公开改写路由不经过这里。

use axum::extract::{Request, State};
use axum::http::HeaderMap;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::http::error::ApiError;
use crate::http::state::AppState;

pub(crate) const HEADER: &str = "x-agent-server-capability";

pub(crate) async fn authorize(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !state.accepts_private_capability(request.headers()) {
        return ApiError::private_access_denied().into_response();
    }
    next.run(request).await
}

pub(crate) fn matches(headers: &HeaderMap, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return false;
    };
    let Some(actual) = headers.get(HEADER).and_then(|value| value.to_str().ok()) else {
        return false;
    };
    constant_time_equal(actual.as_bytes(), expected.as_bytes())
}

fn constant_time_equal(actual: &[u8], expected: &[u8]) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    let mut different = 0u8;
    for (left, right) in actual.iter().zip(expected) {
        different |= left ^ right;
    }
    different == 0
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{HEADER, matches};

    #[test]
    fn requires_an_exact_capability_without_echoing_it() {
        let canary = "capability-canary-do-not-log";
        let mut headers = HeaderMap::new();
        assert!(!matches(&headers, Some(canary)));
        headers.insert(HEADER, HeaderValue::from_static("wrong"));
        assert!(!matches(&headers, Some(canary)));
        headers.insert(
            HEADER,
            HeaderValue::from_static("capability-canary-do-not-log"),
        );
        assert!(matches(&headers, Some(canary)));
        assert!(!matches(&headers, None));
    }
}
