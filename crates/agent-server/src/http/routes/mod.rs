//! 路由表——issue 031 的六个端点 + 会话创建/查询，一比一对应
//! `docs/issues/031-http-sse.md` 的「做什么」小节。每个端点的处理函数在自己的
//! 文件里，这里只做装配。

mod cancel;
mod input;
mod sessions;
mod sse;
mod tool_result;
mod undo;

use axum::Router;
use axum::routing::{get, post};

use crate::http::state::AppState;

pub(in crate::http) fn router(state: AppState) -> Router {
    Router::new()
        .route("/sessions", post(sessions::create))
        .route("/sessions/{id}", get(sessions::status))
        .route("/sessions/{id}/events", get(sse::events))
        .route("/sessions/{id}/input", post(input::input))
        .route("/sessions/{id}/tool_result", post(tool_result::tool_result))
        .route("/sessions/{id}/undo", post(undo::undo))
        .route("/sessions/{id}/redo", post(undo::redo))
        .route("/sessions/{id}/cancel", post(cancel::cancel))
        .with_state(state)
}
