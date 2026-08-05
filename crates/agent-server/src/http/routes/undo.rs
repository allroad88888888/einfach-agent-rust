//! `POST /sessions/:id/undo`、`POST /sessions/:id/redo`——同一对命令，同一个
//! 文件（issue 031 把它们列在一起：「undo/redo/cancel 端点各自生效，复用 030
//! 的命令语义」）。跟 [`crate::http::routes::input`] 一样是 fire-and-forget，
//! 结果走 [`crate::event::SessionEvent::Undo`]/`Redo`。

use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;

use crate::Command;
use crate::command::Granularity;
use crate::http::error::ApiError;
use crate::http::json::ApiJson;
use crate::http::state::AppState;
use crate::registry::SessionId;

/// wire 形状原文（issue 031）：`{ "granularity": "turn"|"step", "force": bool }`。
/// 默认 `granularity: "turn"`（CLI `/undo` 的默认档，决策 5），默认
/// `force: false`。
#[derive(Deserialize, Default)]
#[serde(rename_all = "snake_case")]
enum GranularityWire {
    #[default]
    Turn,
    Step,
}

impl From<GranularityWire> for Granularity {
    fn from(g: GranularityWire) -> Self {
        match g {
            GranularityWire::Turn => Granularity::Turn,
            GranularityWire::Step => Granularity::Step,
        }
    }
}

#[derive(Deserialize)]
pub(in crate::http) struct UndoRequest {
    #[serde(default)]
    granularity: GranularityWire,
    #[serde(default)]
    force: bool,
}

pub(in crate::http) async fn undo(
    State(state): State<AppState>,
    Path(id): Path<String>,
    ApiJson(body): ApiJson<UndoRequest>,
) -> Result<StatusCode, ApiError> {
    // `agent_core::Session` 没有 `undo_step` 的 force 变体（`agent-core/src/
    // command/undo.rs` 模块文档）——这个组合在到达 actor 之前就该被拒绝，而不是
    // 到了那边被 `crate::actor::commands::handle_undo` 悄悄忽略 `force`（防御性
    // 第二道闸留在那边，见它的文档；这里是第一道，也是给客户端一个明确的错误
    // 而不是沉默地做了别的事）。
    if matches!(body.granularity, GranularityWire::Step) && body.force {
        return Err(ApiError::bad_request(
            "granularity: \"step\" 不支持 force：Session 没有 undo_step 的越过屏障变体，只有 turn 粒度有 /undo! 那档",
        ));
    }
    let cmd = Command::Undo {
        granularity: body.granularity.into(),
        force: body.force,
    };
    state.dispatch(&SessionId::from(id), cmd)?;
    Ok(StatusCode::ACCEPTED)
}

pub(in crate::http) async fn redo(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state.dispatch(&SessionId::from(id), Command::Redo)?;
    Ok(StatusCode::ACCEPTED)
}
