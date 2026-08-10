//! `GET /sessions/{id}/compaction_record` 的 JSON 协议类型（109）。
//!
//! 位置照 [`crate::http::pending`]：Rust HTTP 响应与生成的 TypeScript 共用这一份
//! 定义，ts-rs 的 derive 挂在 `ts` feature 门后面。
//!
//! # 它回答的问题
//!
//! 时间线上标出来的压缩点/清除标记只是「发生过」的信号（`SessionEvent::
//! CompactionApplied`/`ToolResultsCleared`，只带 id/turn_id，不带正文）——点开
//! 一条要展开看的**内容**从这里取。109 的接线约束把「有什么」定死在两处：
//!
//! - **完整记录**（`messages`）：`Session::messages_of` 原样翻译，**不经过
//!   `SendPlan`/`project`**——那是「这一轮发什么」，这里要的是「有什么」。
//!   压缩点覆盖的原始轮次（`messages[..upto]`）、被清工具调用的原始结果，
//!   都从这个数组里切/找，不会看到 `CLEARED_TOOL_RESULT` 占位或者摘要替身。
//! - **摘要库**（`summaries`）：`Session::summary_library` 原样翻译，来自
//!   `Slot::Summaries`——生成摘要的那个子 agent 早被 108 回收了，正文只能从
//!   这里取，不能从别处反推。
//!
//! `upto`（切片边界）/`summary_id`（找哪一条摘要）都不在这个响应体里：它们已经
//! 在触发展开的那条 SSE 事件里，前端自己记着，不需要这里重复一份。

use std::sync::Arc;

use serde::Serialize;

use agent_core::{Message, SummaryId};

use crate::actor::message::CompactionRecord;

/// 一份摘要的 id + 正文——`Slot::Summaries` 里一条记录的原样翻译。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct SummaryEntry {
    pub(crate) id: SummaryId,
    pub(crate) text: Arc<str>,
}

/// `GET /sessions/{id}/compaction_record` 的响应体。字段名对齐本模块文档
/// 「它回答的问题」那两条：`messages` 是完整记录，`summaries` 是摘要库，
/// **都不经 `SendPlan`**。
#[derive(Debug, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub(crate) struct CompactionRecordResponse {
    pub(crate) messages: Vec<Message>,
    pub(crate) summaries: Vec<SummaryEntry>,
}

impl From<CompactionRecord> for CompactionRecordResponse {
    fn from(record: CompactionRecord) -> Self {
        CompactionRecordResponse {
            messages: record.messages,
            summaries: record
                .summaries
                .into_iter()
                .map(|(id, text)| SummaryEntry { id, text })
                .collect(),
        }
    }
}
