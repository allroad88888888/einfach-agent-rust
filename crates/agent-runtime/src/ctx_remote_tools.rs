//! [`RunnerCtx`] 持有的远端工具等待槽。
//!
//! 这个状态只能由 actor 线程修改；它把 Web 宿主的回传绑定到已经派出的精确工具
//! 调用，避免 HTTP 客户端伪造 epoch 或写入任意 `ToolsPending` 槽。

use agent_core::{AgentId, Epoch, ToolCallId, ToolCallRequest};

use crate::ctx::RunnerCtx;

/// 已派给远端宿主、尚未获确认的调用。
pub(crate) struct PendingRemoteTool {
    pub(crate) agent: AgentId,
    pub(crate) call_id: ToolCallId,
    pub(crate) epoch: Epoch,
    pub(crate) request: ToolCallRequest,
}

#[derive(Default)]
pub(crate) struct PendingRemoteTools(Vec<PendingRemoteTool>);

impl RunnerCtx {
    /// 登记一个仅能由远端宿主回传的调用。重复 id 违反 provider 协议；保留较早
    /// 的登记，确保任何回传至多收敛一个原始工具槽。
    pub(crate) fn register_remote_tool(
        &mut self,
        agent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        request: ToolCallRequest,
    ) {
        if self
            .pending_remote_tools
            .0
            .iter()
            .any(|pending| pending.agent == agent && pending.call_id == call_id)
        {
            return;
        }
        self.pending_remote_tools.0.push(PendingRemoteTool { agent, call_id, epoch, request });
    }

    /// 只取走确实仍在等待的精确调用；重复、跨 agent 或已撤销的回传都会失败。
    pub(crate) fn take_remote_tool(
        &mut self,
        agent: &AgentId,
        call_id: &ToolCallId,
    ) -> Option<PendingRemoteTool> {
        let index = self
            .pending_remote_tools
            .0
            .iter()
            .position(|pending| &pending.agent == agent && &pending.call_id == call_id)?;
        Some(self.pending_remote_tools.0.remove(index))
    }

    /// 取消、撤回或会话终止后切断未完成远端调用，防止迟到回传写入新 epoch。
    pub fn discard_remote_tools(&mut self) {
        self.pending_remote_tools.0.clear();
    }
}
