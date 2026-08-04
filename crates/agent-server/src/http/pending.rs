//! `GET /sessions/{id}/pending_tools` 的 JSON 协议类型（072）。
//!
//! 位置照 [`crate::http::poll_protocol`]：Rust HTTP 响应与生成的 TypeScript 共用
//! 这一份定义，ts-rs 的 derive 挂在 `ts` feature 门后面。
//!
//! **`Frame`/`SessionEvent` 一个字节不动**——这是选「导出一份独立投影」而不是
//! 「给帧加一个 `replayed` 标记」的直接红利：既有消费者（渲染层、Java 网关、
//! fixtures）全都不用改，`SessionEvent` 的变体数还是 16。

use serde::Serialize;

use agent_core::{AgentId, ToolCallId, ToolCallRequest};
use agent_runtime::RemoteToolWaiting;

/// 一条还欠着宿主回传的远端调用。
///
/// 字段名跟 `SessionEvent::ToolExecuting` 的载荷对齐（`call_id` + `request`）：
/// 宿主的两条路——收到帧、或者连上就拉一次——拿到的是同一种东西，执行那一段代码
/// 因此只有一份。
///
/// **不带 `epoch`**：epoch 是服务端保管的凭据，客户端伪造不了也不该看见
/// （`crate::http::routes::tool_result` 模块文档写死了这条）。**不带截止线**：
/// 那是 060 的内部账，宿主拿它做不了任何正确的决定——它该做的只有「还欠着就执行」。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct PendingTool {
    pub(crate) agent: AgentId,
    pub(crate) call_id: ToolCallId,
    pub(crate) request: ToolCallRequest,
}

/// 投影响应体。用 `{ "pending": [...] }` 这个信封而不是裸数组：将来要给这份
/// 投影加一个 `as_of` 之类的元字段时，不必让所有既有客户端改解析（裸数组没有
/// 任何可扩展的位置）。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct PendingToolsResponse {
    pub(crate) pending: Vec<PendingTool>,
}

impl From<RemoteToolWaiting> for PendingTool {
    fn from(waiting: RemoteToolWaiting) -> Self {
        PendingTool { agent: waiting.agent, call_id: waiting.call_id, request: waiting.request }
    }
}
