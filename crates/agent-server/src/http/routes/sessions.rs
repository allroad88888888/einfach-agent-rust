//! 会话生命周期的两个端点（issue 031「会话创建」）：`POST /sessions` 开一个新
//! session，`GET /sessions/:id` 查它现在是活着还是死了。

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::{SessionId, SessionQuery};

#[derive(Deserialize)]
pub(in crate::http) struct CreateSessionRequest {
    /// `Some` → 落盘（跟 CLI `--session <path>` 同款语义）；`None`/省略 → 内存
    /// 临时会话。
    #[serde(default)]
    session_path: Option<PathBuf>,
}

#[derive(Serialize)]
struct CreateSessionResponse {
    id: String,
}

pub(in crate::http) async fn create(State(state): State<AppState>, ApiJson(body): ApiJson<CreateSessionRequest>) -> Result<Response, ApiError> {
    let id = state.generate_id();
    let spec = state
        .template()
        .open_spec(id.clone(), body.session_path)
        .map_err(|e| ApiError::conflict(format!("session \"{id}\" 的工具根目录建不起来：{e}")))?;
    state.registry().open(spec).map_err(|e| ApiError::conflict(e.to_string()))?;
    // 立刻把 SSE hub 造出来（不等第一次 `GET /events` 才现造）——不然「先
    // `POST /input` 好几轮，稍后才第一次连 SSE」这种顺序会在 hub 存在之前的
    // 这段时间里彻底丢事件（连「补不上」的 gap 都判不出来，因为环形缓冲那时
    // 还不存在）。`hub_for` 内部会先查 registry 确认活着——这里 `open` 刚成功
    // 返回，几乎不可能失败，真失败了也只是这次请求晚一点点再有人重试
    // `GET /events` 时现造，不影响 `POST /sessions` 本身的成功语义。
    let _ = state.hub_for(&id);
    Ok((StatusCode::CREATED, Json(CreateSessionResponse { id: id.to_string() })).into_response())
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(in crate::http) enum SessionStatusResponse {
    Alive,
    Dead { reason: String },
}

pub(in crate::http) async fn status(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<SessionStatusResponse>, ApiError> {
    let id = SessionId::from(id);
    match state.registry().get(&id) {
        None => Err(ApiError::not_found(format!("session \"{id}\" 不存在（从没 open 过，或者已经被 close 摘表）"))),
        Some(SessionQuery::Alive(_)) => Ok(Json(SessionStatusResponse::Alive)),
        Some(SessionQuery::Dead { reason }) => Ok(Json(SessionStatusResponse::Dead { reason })),
    }
}
