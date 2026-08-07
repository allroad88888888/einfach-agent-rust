//! `POST /sessions/:id/input`：一句用户输入。
//!
//! **fire-and-forget**：这个响应只确认「命令送进了 actor 的队列」，不等轮次
//! 跑完——跑完之后发生的一切（增量文本、工具调用、终态）都在 `GET
//! /sessions/:id/events` 上，这是这个传输设计本来的分工（ARCHITECTURE.md
//! §传输：下行 SSE、上行 POST），没有请求-响应关联 id 可以拿来「等这次调用对应
//! 的那条结果」——`Command`/`SessionEvent` 从 030 起就没有这个字段，031 不为了
//! 这一个端点新引入一条关联机制。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::Command;
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::SessionId;

#[derive(Deserialize)]
pub(in crate::http) struct InputRequest {
    text: String,
}

pub(in crate::http) async fn input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<InputRequest>,
) -> Result<StatusCode, ApiError> {
    let session_id = SessionId::from(id);
    // `dispatch` 内部先确认会话活着（404/410 分明）再发命令。纯文本输入没有任何
    // 外部 IO，不需要像图片时代那样在发送前单独预检会话（图片时代那条预检是为
    // 了「不给不存在的会话上传付费文件」，现在没有可付费的副作用了）。
    state.dispatch(&session_id, Command::Input { text: body.text })?;
    Ok(StatusCode::ACCEPTED)
}
