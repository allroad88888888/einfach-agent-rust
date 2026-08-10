//! Internal actor inbox envelope, including non-serializable one-shot replies.

use std::sync::Arc;

use tokio::sync::oneshot;

use agent_core::{AgentId, Message, SummaryId};
use agent_runtime::{
    RemoteToolClaimDecision, RemoteToolClaimRequest, RemoteToolSubmitDecision,
    RemoteToolSubmitRequest,
};

use crate::command::Command;

/// 109：`GET /sessions/{id}/compaction_record` 这次查询要的原始两半——**完整
/// 记录**（`Session::messages_of`，接线约束 1：不经 `SendPlan`/`project`）与
/// **摘要库**（`Session::summary_library`，接线约束 5：从 `Slot::Summaries`
/// 取）。这里只是把 actor 线程上现查出来的两个值原样带出来，不是新类型的
/// wire 形状——那是 `crate::http::compaction::CompactionRecordResponse` 的事
/// （`ClaimRemoteTool` 回复域类型、HTTP 层再翻成协议类型，同一层次）。
pub(crate) struct CompactionRecord {
    pub(crate) messages: Vec<Message>,
    pub(crate) summaries: Vec<(SummaryId, Arc<str>)>,
}

/// Values sent to the session actor. Only [`Command`] is part of the serializable public
/// protocol; remote-tool request/reply messages deliberately keep their one-shot senders here.
pub(crate) enum ActorMessage {
    Command(Command),
    ClaimRemoteTool {
        request: RemoteToolClaimRequest,
        reply: oneshot::Sender<RemoteToolClaimDecision>,
    },
    SubmitRemoteToolResult {
        request: RemoteToolSubmitRequest,
        reply: oneshot::Sender<RemoteToolSubmitDecision>,
    },
    /// 109：读一次这个 agent 的压缩记录。走 mailbox 而不是共享单元格（跟
    /// `tree`/`pending_tools` 不同）——这是用户点开一条压缩标记时才发的一次性
    /// 查询，不是需要在轮次进行中也能立刻拿到「此刻」的活状态，排在已有命令
    /// 后面处理是可接受的延迟，换来的是不必为它另起一套「变了就更新」的回调
    /// 管线。
    ReadCompactionRecord {
        agent: AgentId,
        reply: oneshot::Sender<CompactionRecord>,
    },
}
