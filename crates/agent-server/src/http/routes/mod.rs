//! 路由表——issue 031 的六个端点 + 会话创建/查询，一比一对应
//! `docs/issues/031-http-sse.md` 的「做什么」小节；`GET /sessions/:id/agents`
//! 是 048 补的第七个（整棵活 agent 树此刻的快照，见 `sessions::agents`），
//! `GET /sessions/:id/pending_tools` 是 072 补的（还欠着的远端调用，见
//! `pending_tools`），`GET /sessions/:id/compaction_record` 是 109 补的（展开
//! 压缩点/清除标记要看的完整记录 + 摘要库，见 `compaction::record`）。每个端点
//! 的处理函数在自己的文件里，这里只做装配。

mod cancel;
mod compaction;
mod input;
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

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::{Router, middleware};

use crate::http::state::AppState;

/// `POST /sessions/:id/input` 的 JSON 请求体上限。issue 091 之后输入只有纯文本
/// （图片不再经这条路上传），1 MiB 对一句用户输入绰绰有余，同时比 axum 默认的
/// 2 MiB 更紧，保留显式边界而不是悄悄依赖框架默认值。
const INPUT_BODY_LIMIT_BYTES: usize = 1024 * 1024;

pub(in crate::http) fn router(state: AppState) -> Router {
    let mut router = Router::new()
        .route("/sessions", post(sessions::create))
        .route("/sessions/{id}", get(sessions::status))
        .route("/sessions/{id}/agents", get(sessions::agents))
        .route("/sessions/{id}/pending_tools", get(pending_tools::list))
        .route("/sessions/{id}/compaction_record", get(compaction::record))
        .route("/sessions/{id}/events", get(sse::events))
        .route("/sessions/{id}/events/poll", get(poll::events))
        .route(
            "/sessions/{id}/input",
            post(input::input).layer(DefaultBodyLimit::max(INPUT_BODY_LIMIT_BYTES)),
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
        .route("/sessions/{id}/cancel", post(cancel::cancel));
    // s5：配了 `upload_dir` 才挂上传端点——没配就不存在这两个路由，跟
    // 部署方根本没开图片上传的旧行为逐字节一致。body 上限放开到
    // `MAX_IMAGE_BYTES`（100 MiB，跟 transport 侧 Moonshot 同一上限），
    // 不能沿用纯文本输入那 1 MiB。
    if state.uploads_enabled() {
        router = router
            .route(
                "/uploads",
                post(crate::http::uploads::upload)
                    .layer(DefaultBodyLimit::max(agent_transport::MAX_IMAGE_BYTES)),
            )
            .route("/uploads/{id}", get(crate::http::uploads::get));
    }
    router
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::http::private_capability::authorize,
        ))
        .with_state(state)
}
