//! `POST /sessions/:id/tool_result`：前端工具（`location: Web`，见
//! `docs/TOOLS.md`）是 033 之后才落地的事——M3 这里先诚实地 501，**不是 404**
//! （issue 031 原文要求这两者分明：客户端要能分清「这条路不存在」和「这条路
//! 存在、这一版没做」）。不检查 `id` 是否存在——这条路径本身就没准备好接住
//! 任何调用，去查 session 状态只会制造一个多余的 404/410 分支，掩盖「这个端点
//! 压根没启用」这个更根本的事实。

use crate::http::error::ApiError;

pub(in crate::http) async fn tool_result() -> ApiError {
    ApiError::not_implemented("POST /tool_result 这一版还没启用：前端工具（location: Web）是 033 之后的事，M3 只有 srv 工具本地执行".to_string())
}
