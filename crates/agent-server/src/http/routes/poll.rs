//! `GET /sessions/{id}/events/poll`：同一事件 ring 的拉取式投影。
//!
//! 游标仍用 SSE 的 `Last-Event-ID`；可选的 `X-Poll-Wait-Ms` 把空批升级为长
//! 轮询。请求的整个存活期持有 `SubscriberGuard`，所以它与 SSE 共享取消宽限。

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;

use crate::http::error::ApiError;
use crate::http::hub::{BufferedFrame, SubscriberGuard};
use crate::http::poll_protocol::{PollFrame, PollResponse};
use crate::http::state::AppState;
use crate::registry::SessionId;

const LAST_EVENT_ID: &str = "last-event-id";
const POLL_WAIT_MS: &str = "x-poll-wait-ms";

pub(in crate::http) async fn events(
    State(state): State<AppState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<PollResponse>, ApiError> {
    let id = SessionId::from(id);
    let hub = state.hub_for(&id)?;
    let last_event_id = cursor(&headers);
    let wait = wait_duration(&headers);

    // 从请求进入到响应交还给 axum 都持有 guard。特别是长轮询的 await 期间，
    // 不能因为没有 SSE 连接就误触发取消宽限。
    let _subscriber = SubscriberGuard::attach(Arc::clone(&hub));
    let (initial, mut live_rx) = hub.replay_and_subscribe(last_event_id);
    let frames = if initial.is_empty() && !wait.is_zero() {
        await_new_frames(&hub, last_event_id, wait, &mut live_rx).await
    } else {
        initial
    };

    let next = frames
        .last()
        .map_or(last_event_id.unwrap_or(0), |frame| frame.id);
    Ok(Json(PollResponse {
        frames: frames.into_iter().map(to_poll_frame).collect(),
        next,
    }))
}

fn cursor(headers: &HeaderMap) -> Option<u64> {
    headers
        .get(LAST_EVENT_ID)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn wait_duration(headers: &HeaderMap) -> Duration {
    let millis = headers
        .get(POLL_WAIT_MS)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    Duration::from_millis(millis)
}

async fn await_new_frames(
    hub: &crate::http::hub::SseHub,
    last_event_id: Option<u64>,
    wait: Duration,
    live_rx: &mut tokio::sync::broadcast::Receiver<BufferedFrame>,
) -> Vec<BufferedFrame> {
    // 收到一条 live 通知时重新读 ring，而不是只交付这一个通知：这一小段内可能
    // 已经连续产生多帧，重读让响应保持一个完整、同源的 replay 批次。
    let _ = tokio::time::timeout(wait, live_rx.recv()).await;
    hub.replay_frames(last_event_id)
}

fn to_poll_frame(frame: BufferedFrame) -> PollFrame {
    PollFrame {
        id: frame.id,
        event: frame.event,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_headers_degrade_to_an_immediate_first_poll() {
        let mut headers = HeaderMap::new();
        headers.insert(LAST_EVENT_ID, "not-a-number".parse().unwrap());
        headers.insert(POLL_WAIT_MS, "also-not-a-number".parse().unwrap());
        assert_eq!(cursor(&headers), None);
        assert!(wait_duration(&headers).is_zero());
    }
}
