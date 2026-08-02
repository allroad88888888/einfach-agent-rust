//! `GET /sessions/:id/events`：SSE 下行——`Frame`（034 起的 agent 归属信封，
//! `crate::event::frame` 模块文档）逐帧 + 心跳（issue 031）。真正的补发/直播/
//! 断开取消逻辑全部在 [`crate::http::hub`]，这个文件只做三件事：解析
//! `Last-Event-ID`、把 hub 给的 `mpsc::Receiver` 包成 axum 的 SSE `Stream`、
//! 在响应上钉死那两个 header。

use std::convert::Infallible;

use axum::extract::{Path, State};
use axum::http::{HeaderName, HeaderValue, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use tokio_stream::StreamExt;
use tokio_stream::wrappers::ReceiverStream;

use crate::http::error::ApiError;
use crate::http::hub::BufferedFrame;
use crate::http::state::AppState;
use crate::registry::SessionId;

/// 这两个 header 是 ARCHITECTURE.md §传输 的硬要求：企业中间层（nginx /
/// Ingress / 内部 LB）默认缓冲会把流式响应变成「一次性吐完」，server 一次发对
/// 全链路才老实。
const HEADER_NO_ACCEL_BUFFERING: (&str, &str) = ("x-accel-buffering", "no");

pub(in crate::http) async fn events(State(state): State<AppState>, Path(id): Path<String>, headers: axum::http::HeaderMap) -> Result<Response, ApiError> {
    let id = SessionId::from(id);
    let hub = state.hub_for(&id)?;

    let last_event_id = headers.get("last-event-id").and_then(|v| v.to_str().ok()).and_then(|s| s.parse::<u64>().ok());

    let (rx, guard) = hub.spawn_forwarder(last_event_id);
    // `guard` 被这个 `.map` 闭包整个拿走（`move`）——它的存活期从此就是这个
    // `Stream` 对象的存活期：axum/hyper 检测到客户端断开时会丢弃这整条
    // `Stream`（不需要它先产出过任何东西），`guard` 跟着 drop，宽限计时器
    // 从这一刻开始倒数。见 `crate::http::hub` 模块文档「`SubscriberGuard`
    // 为什么不能活在转发任务里」——那是这个写法要避免重蹈的独测事故。
    let stream = ReceiverStream::new(rx).map(move |frame| {
        let _keep_guard_alive = &guard;
        Ok::<Event, Infallible>(to_sse_event(&frame))
    });
    let sse = Sse::new(stream).keep_alive(KeepAlive::new().interval(state.sse_keep_alive()).text("keep-alive"));

    let mut response = sse.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(HeaderName::from_static(HEADER_NO_ACCEL_BUFFERING.0), HeaderValue::from_static(HEADER_NO_ACCEL_BUFFERING.1));
    Ok(response)
}

fn to_sse_event(frame: &BufferedFrame) -> Event {
    // 红线 3 的精神：`Frame`（034 的 agent 归属信封，内层 `SessionEvent`）全部
    // 可序列化，这里的 `expect` 不是「祈祷不出错」，是这份契约的运行期断言——
    // 真出错说明红线被绕过了，不该假装成功发一帧空的出去。SSE 帧的 `data:` 从
    // 034 起就是 `{"agent":"...","event":{"type":"...","data":...}}`——信封本身
    // 序列化，不是只序列化里面那个 `event`。
    let data = serde_json::to_string(&frame.event).expect("Frame 全部可序列化（红线 3）");
    Event::default().id(frame.id.to_string()).data(data)
}
