//! `GET /sessions/{id}/compaction_record`（109）：见 `crate::http::compaction`
//! 模块文档「它回答的问题」——展开一条压缩标记要看的完整记录 + 摘要库。
//!
//! `agent` 走**查询参数**而不是路径段：`AgentId` 可能含 `/`（`root/a1`），塞进
//! URL 路径段会被当成额外的路由层级切开；查询参数原样带着走，前端只需要
//! `encodeURIComponent`。省略时默认 `root`——今天只有 root 会触发压缩（096/108
//! 「只判 root」的既有决定），子 agent 压缩若哪天上线，这里再加分支不难。

use axum::Json;
use axum::extract::{Path, Query, State};
use serde::Deserialize;

use agent_core::AgentId;

use crate::http::compaction::CompactionRecordResponse;
use crate::http::error::ApiError;
use crate::http::state::AppState;
use crate::registry::SessionId;

#[derive(Deserialize)]
pub(in crate::http) struct CompactionRecordQuery {
    agent: Option<String>,
}

pub(in crate::http) async fn record(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<CompactionRecordQuery>,
) -> Result<Json<CompactionRecordResponse>, ApiError> {
    let agent = query.agent.map_or_else(AgentId::root, AgentId::new);
    let handle = state.session_handle(&SessionId::from(id))?;
    let reply = handle
        .read_compaction_record(agent)
        .map_err(|_| ApiError::gone("session 的 actor 线程在这条查询送达之前已经不在了"))?;
    let record = reply
        .await
        .map_err(|_| ApiError::gone("session 的 actor 在线程间请求完成前已经停止"))?;
    Ok(Json(record.into()))
}
