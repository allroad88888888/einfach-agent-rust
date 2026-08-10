//! 109：`SessionHandle` 上「查一次压缩记录」的那一半。跟 [`crate::
//! handle_remote_tools`] 同一种分法——请求/回复方法各自成一个文件，`handle.rs`
//! 只留 `SessionHandle` 本体。

use tokio::sync::oneshot;

use agent_core::AgentId;

use crate::actor::message::{ActorMessage, CompactionRecord};
use crate::handle::{SessionClosed, SessionHandle};

impl SessionHandle {
    /// 排进 actor 的命令队列，回来时拿到这个 agent 的完整记录 + 摘要库
    /// （见 [`CompactionRecord`] 文档「原始两半」）。
    pub(crate) fn read_compaction_record(
        &self,
        agent: AgentId,
    ) -> Result<oneshot::Receiver<CompactionRecord>, SessionClosed> {
        let (reply, response) = oneshot::channel();
        self.enqueue(ActorMessage::ReadCompactionRecord { agent, reply })?;
        Ok(response)
    }
}
