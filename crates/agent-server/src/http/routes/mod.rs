//! 路由表——issue 031 的六个端点 + 会话创建/查询，一比一对应
//! `docs/issues/031-http-sse.md` 的「做什么」小节；`GET /sessions/:id/agents`
//! 是 048 补的第七个（整棵活 agent 树此刻的快照，见 `sessions::agents`），
//! `GET /sessions/:id/pending_tools` 是 072 补的（还欠着的远端调用，见
//! `pending_tools`）。每个端点的处理函数在自己的文件里，这里只做装配。

mod cancel;
mod input;
mod input_limits;
mod pending_tools;
mod poll;
mod remote_tool_actor;
mod remote_tool_validation;
mod sessions;
mod sse;
mod tool_claim;
mod tool_result;
mod tool_status;
mod undo;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};

use crate::http::state::AppState;

pub(in crate::http) fn router(state: AppState) -> Router {
    Router::new()
        .route("/sessions", post(sessions::create))
        .route("/sessions/{id}", get(sessions::status))
        .route("/sessions/{id}/agents", get(sessions::agents))
        .route("/sessions/{id}/pending_tools", get(pending_tools::list))
        .route("/sessions/{id}/events", get(sse::events))
        .route("/sessions/{id}/events/poll", get(poll::events))
        .route(
            "/sessions/{id}/input",
            post(input::input).layer(DefaultBodyLimit::max(input_limits::INPUT_BODY_LIMIT_BYTES)),
        )
        .route("/sessions/{id}/tool_claim", post(tool_claim::claim))
        .route(
            "/sessions/{id}/tool_result",
            post(tool_result::tool_result).layer(DefaultBodyLimit::max(
                remote_tool_validation::MAX_TOOL_RESULT_BODY_BYTES,
            )),
        )
        .route("/sessions/{id}/tool_status", get(tool_status::status))
        .route("/sessions/{id}/undo", post(undo::undo))
        .route("/sessions/{id}/redo", post(undo::redo))
        .route("/sessions/{id}/cancel", post(cancel::cancel))
        .with_state(state)
}
