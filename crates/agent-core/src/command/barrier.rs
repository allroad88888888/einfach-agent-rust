//! [`Session::barrier_info`]：从一条屏障 entry 里抠出「越过它意味着什么」
//! （034）——`/undo!` 越过一条 `EntryMeta.barrier = true` 的 entry 之前，用户
//! 该看到工具名 + call_id，不是一个裸的 `barrier_seq` 数字（027 的原则：
//! 让人明白自己在确认什么）。
//!
//! 一份实现两处用：`agent-cli` 的 `/undo!` 确认文案（`agent_cli::undo::
//! describe_barrier`）与 `agent-server` 的 `UndoOutcome::Blocked` 富化都走这个
//! 读口——034 把原来只活在 CLI 里的私有逻辑搬到这里（CLI 与 server 各自跟着换用
//! 公共读口，行为不变）。独立成一个文件（而不是塞进 [`super::read`]）：这是
//! 「描述一条屏障 entry」这一件事，`read` 那批 `*_of(agent)` 口子答的是另一个
//! 问题（宿主替某个 agent 取它自己的槽位），两者不该挤在同一个文件里（红线 9
//! 精神）。

use std::sync::Arc;

use crate::engine::state::SlotState;
use crate::ids::ToolCallId;

use super::meta::AgentEntry;
use super::session::Session;

/// 一条屏障（[`crate::EntryMeta::barrier`] 为真）的描述——[`Session::barrier_info`]
/// 的返回值。
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BarrierInfo {
    /// 这条 entry 是什么（`EntryMeta.label`）。
    pub label: &'static str,
    /// 若这条屏障来自一次工具结果，工具名——**目前恒为 `Some`**：`barrier`
    /// 只会在 `tool_result`/`tool_failed` 那条上置真（`Session::mark_irreversible`
    /// 的唯一调用点是宿主派发工具时）。`None` 是防御性的兜底，不是已知会走到
    /// 的分支。
    pub tool: Option<Arc<str>>,
    pub call_id: Option<ToolCallId>,
}

impl Session {
    /// 按 `seq` 找一条 entry，抠出它的屏障描述。
    ///
    /// `None` = history 里没有这个 `seq`（理论上不该发生：调用方手上的 `seq`
    /// 该来自 `UndoReport::Blocked::barrier_seq`，那正是 `undo_turn` 扫描时
    /// 真实停在的那一条）。
    pub fn barrier_info(&self, seq: u64) -> Option<BarrierInfo> {
        let entry = self.history.entries().find(|e| e.seq == seq)?;
        let (tool, call_id) = tool_barrier_of(entry);
        Some(BarrierInfo {
            label: entry.meta.label,
            tool,
            call_id,
        })
    }
}

/// 从一条 entry 的 `ToolSlots` 变更里找出「哪个槽从 `Pending` 变 `Finished`」，
/// 抠出工具名 + call_id。找不到（entry 不是工具结果那种形状——理论上不该
/// 发生，`barrier` 只会在 `tool_result`/`tool_failed` 那条上）就兜底
/// `(None, None)`，不 panic。
fn tool_barrier_of(entry: &AgentEntry) -> (Option<Arc<str>>, Option<ToolCallId>) {
    for change in &entry.changes {
        let (Some(prev), Some(next)) = (change.prev.as_slots(), change.next.as_slots()) else {
            continue;
        };
        for (p, n) in prev.iter().zip(next.iter()) {
            if matches!(p.state, SlotState::Pending)
                && matches!(n.state, SlotState::Finished { .. })
            {
                return (Some(n.tool.clone()), Some(n.call_id.clone()));
            }
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::engine::Event;
    use crate::ids::AgentId;
    use crate::seam::PrefixImage;
    use crate::value::message::ContentBlock;
    use crate::value::session::{StopReason, TokenUsage};

    use super::*;

    /// 同 `agent-cli::undo` 里那份夹具（同一个真实序列：先声明一次
    /// `srv:shell/exec` 调用、宿主标记不可逆、结果落地），钉住 `barrier_info`
    /// 真的能从一条 barrier entry 里抠出工具名和 call_id——搬迁没有改变行为。
    fn session_with_a_barrier_entry() -> Session {
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: "跑个命令".into(),
            images: Vec::new(),
        });
        let call_id = ToolCallId::new("call_shell_1");
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::ToolUse {
                id: call_id.clone(),
                name: Arc::from("srv:shell/exec"),
                input: Arc::new(serde_json::json!({"cmd": "echo hi"})),
            }],
            stop: StopReason::ToolUse,
            usage: TokenUsage {
                prompt: 10,
                completion: 5,
                cached: None,
            },
            prefix: PrefixImage {
                segments: Vec::new(),
                prompt_tokens: None,
            },
            adjustments: Vec::new(),
        });
        session.mark_irreversible(call_id.clone());
        let _ = session.step(Event::ToolResult {
            agent: AgentId::root(),
            epoch: session.epoch(),
            call_id,
            content: Arc::from("hi\n"),
        });
        session
    }

    #[test]
    fn barrier_info_extracts_the_tool_name_and_call_id() {
        let session = session_with_a_barrier_entry();
        let entry = session.last_entry().unwrap();
        assert!(
            entry.meta.barrier,
            "标记过 mark_irreversible，这条 entry 该带 barrier"
        );

        let info = session
            .barrier_info(entry.seq)
            .expect("这条 seq 真的在 history 里");
        assert_eq!(info.tool.as_deref(), Some("srv:shell/exec"));
        assert_eq!(
            info.call_id.map(|c| c.0.to_string()),
            Some("call_shell_1".to_string())
        );
    }

    #[test]
    fn an_unknown_seq_is_none() {
        let session = session_with_a_barrier_entry();
        assert!(session.barrier_info(9_999).is_none());
    }
}
