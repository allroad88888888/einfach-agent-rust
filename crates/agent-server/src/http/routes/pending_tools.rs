//! `GET /sessions/{id}/pending_tools`（072）：此刻还欠着宿主回传的远端调用。
//!
//! # 它回答的问题
//!
//! 「这次 `web:` 调用现在还要不要我执行？」——**唯一权威的判据**。宿主收到一帧
//! `tool_executing` 无法自己回答它：那帧可能是刚派下来的活，也可能是 ring 补发的
//! 历史（同一个 chatid 上换一个没有游标的新客户端就会拿到整个 ring，M9 的拉取式
//! 网关每次浏览器刷新都是这个形状）。而「是不是补发」本身也不是正确的判据——派了
//! 活、宿主还没执行就换了客户端，那帧确实是补发的、活却真的还欠着。
//!
//! 于是判据落在服务端状态上：**还在等待槽里就该执行，不在就不该**。刷新掉不了
//! 服务端状态，所以这条判据对任何宿主（浏览器、Java 网关、明天第三种）都成立，
//! 不需要每个集成方各实现一遍客户端存储。
//!
//! 数据源是 `RunnerCtx` 里那张等待槽表**本身**（经
//! [`crate::handle::SessionHandle::pending_remote_tools`] 那个共享单元格），不是
//! 第二份账——「不新增第二真值源」（INTEGRATION.md §七）。
//!
//! # 它**不**做的事
//!
//! **不改 ring**。那条早已收场的 `tool_executing` 照旧原样补发——ring 是日志，
//! 「要不要执行」是客户端的决定，不该被编码进服务端下发的历史（渲染层想画「当时
//! 确实调过」、审计在别处重放同一段，拿到的都该是完整的过去）。
//!
//! # 死会话 410、休眠会话 404
//!
//! 跟 `GET /sessions/:id/agents` 同一条判据（[`crate::http::state::AppState::
//! session_handle`] 这一个函数），不是 [`crate::http::routes::sessions::status`]
//! 那条「问问这个 id 现在死没死」的三态路：等待槽是**运行时状态**，活在 actor
//! 线程手上，actor 没了槽就没了。所以磁盘上有历史但此刻没活着的会话（073 的
//! `dormant`）在这里是 404 而不是「空投影」——答一个空数组等于告诉宿主「你不欠
//! 任何活」，而真相是「这个会话现在根本没在跑，问都问不到」，两者差着一次误判。

use axum::Json;
use axum::extract::{Path, State};

use crate::http::error::ApiError;
use crate::http::pending::{PendingTool, PendingToolsResponse};
use crate::http::state::AppState;
use crate::registry::SessionId;

pub(in crate::http) async fn list(State(state): State<AppState>, Path(id): Path<String>) -> Result<Json<PendingToolsResponse>, ApiError> {
    let handle = state.session_handle(&SessionId::from(id))?;
    let pending = handle.pending_remote_tools().into_iter().map(PendingTool::from).collect();
    Ok(Json(PendingToolsResponse { pending }))
}
