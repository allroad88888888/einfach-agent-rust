//! 会话的只读状态视图：生命周期状态与当前 agent 树。

use agent_core::AgentTree;
use axum::Json;
use axum::extract::{Path, State};
use serde::Serialize;

use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::registry::{SessionId, SessionQuery};

#[derive(Serialize)]
#[serde(rename_all = "snake_case", tag = "status")]
pub(in crate::http) enum SessionStatusResponse {
    Alive,
    Dead {
        reason: String,
    },
    /// registry 里没有，但磁盘上有会话文件；下一次创建将恢复。
    Dormant,
}

/// `GET /sessions/:id`：404 仅表示该 chatid 没有活会话也没有历史。
pub(in crate::http) async fn status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<SessionStatusResponse>, ApiError> {
    let id = SessionId::from(id);
    match state.registry().get(&id) {
        Some(SessionQuery::Alive(_)) => Ok(Json(SessionStatusResponse::Alive)),
        Some(SessionQuery::Dead { reason }) => Ok(Json(SessionStatusResponse::Dead { reason })),
        None if state
            .template()
            .default_session_path(&id)
            .is_some_and(|path| path.is_file()) =>
        {
            Ok(Json(SessionStatusResponse::Dormant))
        }
        None => Err(ApiError::not_found(format!(
            "session \"{id}\" 不存在（从没 open 过，也没有留下会话文件）"
        ))),
    }
}

/// `GET /sessions/:id/agents`：从共享快照读取当前活 agent 树，不排 actor 命令队列。
pub(in crate::http) async fn agents(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<AgentTree>, ApiError> {
    let handle = state.session_handle(&SessionId::from(id))?;
    Ok(Json(handle.agent_tree()))
}
