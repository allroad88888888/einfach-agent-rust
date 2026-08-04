//! [`ApiError`]：这个 crate 唯一的 HTTP 错误形状——`{"error":{"code","message"}}`
//! （issue 031「错误形状统一 JSON」）。每个路由处理函数的 `Err` 分支都用这个
//! 类型，`IntoResponse` 是唯一一处知道怎么把它翻成状态码 + JSON 的地方，避免
//! 每个路由各写各的错误体格式。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// 这个 session id 从没 `open` 过。
    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::NOT_FOUND, code: "session_not_found", message: message.into() }
    }

    /// 这个 session 的 actor 已经死了（panic）——**410 不是 404**：id 存在过，
    /// 只是不在了，跟「压根没这个资源」是两种诚实（issue 031「404/409/410
    /// （dead）分明」）。
    pub fn gone(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::GONE, code: "session_dead", message: message.into() }
    }

    /// 跟现有状态冲突——目前唯一的来源是 `SessionRegistry::open` 的
    /// `OpenError`（同一个 id 正在被开或者还活着，参见 `crate::registry` 模块
    /// 文档）。031 生成的 id 是进程内单调的，实践中不会真的撞上，但接口层的
    /// 语义仍然要对：`open()` 失败就是 409，不是别的。
    pub fn conflict(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::CONFLICT, code: "session_conflict", message: message.into() }
    }

    /// 请求体本身不合法（比如 `granularity: "step"` 和 `force: true` 这个
    /// `agent_core::Session` 压根没有对应方法的组合）。
    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::BAD_REQUEST, code: "bad_request", message: message.into() }
    }

    /// 073：这个 chatid 已经有历史了，还带着 `capabilities` 来建会话——**拒绝**。
    /// 能力属于历史，历史不接受改写（`docs/HOST-CAPABILITIES.md` §三）。
    ///
    /// **为什么不复用 `bad_request`**：调用方必须能把「我名字写错了」（改一下重发
    /// 就行）和「这个会话已有历史」（不该带声明，去掉它重发）分开——两者都是 400，
    /// 光看状态码分不出来，而它们的正确应对完全相反。所以这一条有自己的
    /// `code`（`session_has_history`），这是本 issue 欠客户端的那个可判别错误码。
    pub fn session_has_history(message: impl Into<String>) -> Self {
        ApiError { status: StatusCode::BAD_REQUEST, code: "session_has_history", message: message.into() }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": { "code": self.code, "message": self.message } }));
        (self.status, body).into_response()
    }
}
