//! 拉取式事件端点的 JSON 协议类型。
//!
//! 这两种类型是 Rust HTTP 响应与生成的 TypeScript 类型共用的一份定义；事件
//! 本身继续复用现有的 [`crate::Frame`] 协议信封。

use serde::Serialize;

use crate::Frame;

#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct PollFrame {
    pub(crate) id: u64,
    pub(crate) event: Frame,
}

/// 长轮询响应；`next` 是下一次请求应带的最后已交付帧 id。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct PollResponse {
    pub(crate) frames: Vec<PollFrame>,
    pub(crate) next: u64,
}
