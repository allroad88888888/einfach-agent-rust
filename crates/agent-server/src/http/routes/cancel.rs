//! `POST /sessions/:id/cancel`：旁路取消当前在飞的轮次（[`Command::Cancel`]，
//! 030 的旁路语义——不排队，见 `crate::command` 模块文档）。这是给用户主动点
//! 「停止」用的端点；SSE 连接断开触发的取消走的是另一条路
//! （[`crate::http::hub`] 的宽限计时器），两者最终都是同一个
//! `SessionHandle::cancel`。

use axum::extract::{Path, State};
use axum::http::StatusCode;

use crate::Command;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::registry::SessionId;

pub(in crate::http) async fn cancel(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.dispatch(&SessionId::from(id), Command::Cancel)?;
    Ok(StatusCode::ACCEPTED)
}
