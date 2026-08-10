//! 052（后台 spawn）端到端用例的共用件：029 那份并发假服务器 / `RunnerCtx` 装配 /
//! SSE 脚本生成**原样复用**（`spawn_indep_support`），外加几个「怎么读一次
//! run_turn 的结果」的断言助手。
//!
//! 复用而不是重写的理由跟 051 的 `status_indep_support` 一字不差：那份夹具里没有
//! 一处是 spawn 专属的，而这几个用例要的正是同一种服务器形状（一条连接一个线程，
//! 父的下一跳和子的第一跳真的同时到达）。
#![allow(dead_code, unused_imports)]

#[path = "../spawn_indep_support/mod.rs"]
pub mod base;

pub use base::{
    build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir, wire_tool_name, Route,
    RoutedServer, SPAWN_WIRE,
};

use agent_core::{AgentId, ContentBlock, Session};
use agent_runtime::{AgentEvent, OrphanFate, RunnerEvent};

/// 某个 agent 历史里全部 tool_result：`(call_id, 正文, is_error)`，按出现顺序。
///
/// 后台 spawn 的验收有一半是「**没有**多出来的那一条」——数量和 call_id 都要看，
/// 只 `contains` 一段正文的话，多回写一条幽灵结果也照样绿。
pub fn tool_results(session: &Session, agent: &AgentId) -> Vec<(String, String, bool)> {
    session
        .messages_of(agent)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult {
                id,
                content,
                is_error,
            } => Some((id.0.to_string(), content.to_string(), *is_error)),
            _ => None,
        })
        .collect()
}

/// 整个会话里（root + 每一个曾经活过的 agent）有没有哪条消息带上了 `needle`。
///
/// 「幽灵结果没进世界」这条断言必须扫**全部** agent，不能只扫 root：一个只在子
/// agent 历史里落地的幽灵照样是落地了。
pub fn any_message_mentions(session: &Session, agents: &[AgentId], needle: &str) -> bool {
    agents.iter().any(|agent| {
        session.messages_of(agent).iter().any(|m| {
            m.blocks.iter().any(|block| match block {
                ContentBlock::Text(t) => t.contains(needle),
                ContentBlock::Thinking(t) => t.contains(needle),
                ContentBlock::ToolResult { content, .. } => content.contains(needle),
                ContentBlock::ToolUse { input, .. } => input.to_string().contains(needle),
            })
        })
    })
}

/// 轮末孤儿告警里有没有点名 `child` 这个后台子 agent。
///
/// 054 起走的是专属变体 `RunnerEvent::OrphanedChild`（052 借的是
/// `TransportTrouble`，那个名字对不上语义，见 `agent_runtime::orphan` 模块
/// 文档）——于是这个助手比对的是**结构化的 `child` 字段**，不再是一段文本里
/// `contains` 一个 id 子串：文案改一个字这条断言就不该跟着红。
pub fn warned_about(events: &[AgentEvent], child: &str) -> bool {
    orphan_warnings(events).iter().any(|(id, _)| id == child)
}

/// 全部轮末孤儿告警：`(出事的子 agent, 怎么收场的)`，按发出顺序。
pub fn orphan_warnings(events: &[AgentEvent]) -> Vec<(String, OrphanFate)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunnerEvent::OrphanedChild { child, fate } => {
                Some((child.as_str().to_string(), fate.clone()))
            }
            _ => None,
        })
        .collect()
}

/// 某个 agent 的流式可见文本增量拼起来——「那条在飞的结果**真的回来了**」的证据
/// （043 的 `mcp_epoch_writeback` 用 `ToolExecuted` 钉同一件事）。
pub fn streamed_text(events: &[AgentEvent], agent: &AgentId) -> String {
    events
        .iter()
        .filter(|e| &e.agent == agent)
        .filter_map(|e| match &e.event {
            RunnerEvent::TextDelta(t) => Some(&**t),
            _ => None,
        })
        .collect()
}
