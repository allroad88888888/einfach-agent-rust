//! 206 独立测试的共用件：029 那份并发假服务器 / `RunnerCtx` 装配 / SSE 脚本生成
//! **原样复用**（`spawn_indep_support`），外加几个「怎么读一次 `run_turn` 之后
//! 谁的历史里有什么、在第几条」的断言助手。
//!
//! 复用而不是重写的理由跟 051 的 `status_indep_support` 一字不差：那份夹具里
//! 没有一处是 spawn 专属的，而 206 要的正是同一种服务器形状——一条连接一个线程，
//! 两个兄弟的请求真的并行到达，而且**能让某一路慢下来制造在飞窗口**
//! （`Route::delay`，206 最硬那条断言全靠它）。
//!
//! # 这份测试的黑盒来源
//!
//! docs/issues/206-send-tool-and-wakeup.md（规格本体）、
//! docs/issues/204-agent-mesh-decision.md §二（背景决策）、
//! docs/INVARIANTS.md 红线 2 / 6 / 11、以及 `agent_runtime` / `agent_core`
//! **导出面上的签名与 rustdoc**（`SEND_TOOL` / `send_spec` / `ToolTable::with_send`
//! / `RunnerEvent::UnreadMessages` / `Session::inbox_of` 等）。
//!
//! **实现体一行没读**：`send_tool.rs`、`unread_inbox.rs`、`dispatch.rs`、
//! `runner.rs`、`agent-core` 的 `command/inbox.rs` 五个文件全程没打开
//! （WORKFLOW §三：看了实现，测的就只剩实现想到的那几条路径）。
#![allow(dead_code, unused_imports)]

// 同 `spawn_bg_support` / `status_indep_support` 那条：有意的重复加载，
// 去重会把两边的 `RoutedServer` 合成同一个类型身份，那是夹具结构的改动。
#[allow(clippy::duplicate_mod)]
#[path = "../spawn_indep_support/mod.rs"]
pub mod base;

pub use base::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir,
    wire_tool_name,
};

use agent_core::{AgentId, ContentBlock, Role, Session};
use agent_runtime::{AgentEvent, RunnerEvent};

/// `srv:agent/send` 的 wire 形式。脚本里每个文件都要用，焊在这里避免每处重算
/// （`wire_tool_name` 自己在 `base` 里有一条对照已知转义的自检）。
pub const SEND_WIRE: &str = "srv_3Aagent_2Fsend";

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`。
///
/// **按 call_id 取，不按顺序取**：206 的拒绝用例一条 assistant 消息里并列六个
/// 调用，顺序断言会在多加一个调用时静默错位到隔壁那条结果上。
pub fn tool_result(session: &Session, agent: &AgentId, call_id: &str) -> (String, bool) {
    session
        .messages_of(agent)
        .iter()
        .flat_map(|m| m.blocks.iter())
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                id,
                content,
                is_error,
            } if &*id.0 == call_id => Some((content.to_string(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "{} 的历史里没有 call_id={call_id} 的 tool_result",
                agent.as_str()
            )
        })
}

/// 一条消息的全部正文拼起来——找 needle 用，四种块都算。
fn message_text(message: &agent_core::Message) -> String {
    message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::Text(t) | ContentBlock::Thinking(t) => t.to_string(),
            ContentBlock::ToolResult { id, content, .. } => format!("{}{content}", id.0),
            ContentBlock::ToolUse { id, name, input } => format!("{}{name}{input}", id.0),
        })
        .collect()
}

/// 某个 agent 的 `Messages` 里，第一条含 `needle` 的消息下标。
pub fn index_of(session: &Session, agent: &AgentId, needle: &str) -> Option<usize> {
    session
        .messages_of(agent)
        .iter()
        .position(|m| message_text(m).contains(needle))
}

/// **排空进来的那条长什么样**：`Role::User` 的一条消息、单个
/// `ContentBlock::Text`（206「做什么」§2 与 205 的既有形状）。返回 `(下标, 正文)`。
///
/// 形状在这里断言一次，正文的性质（含发信人路径 id、含原文、顺序）交给各用例
/// ——**整个格式串不抄死**，那是文案不是契约。
pub fn injected(session: &Session, agent: &AgentId, needle: &str) -> (usize, String) {
    let messages = session.messages_of(agent);
    let idx = messages
        .iter()
        .position(|m| {
            m.role == Role::User
                && m.blocks.len() == 1
                && matches!(&m.blocks[0], ContentBlock::Text(t) if t.contains(needle))
        })
        .unwrap_or_else(|| {
            panic!(
                "{} 的历史里没有一条「单个 Text 块的 user 消息」含 {needle}：{:#?}",
                agent.as_str(),
                messages
            )
        });
    match &messages[idx].blocks[0] {
        ContentBlock::Text(t) => (idx, t.to_string()),
        other => panic!("刚判过是 Text，得到 {other:?}"),
    }
}

/// 带 `ToolUse{id: call_id}` 的那条 assistant 消息的下标——「那次在飞请求的
/// assistant 回复」在历史里的位置。
pub fn tool_use_index(session: &Session, agent: &AgentId, call_id: &str) -> usize {
    session
        .messages_of(agent)
        .iter()
        .position(|m| {
            m.blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::ToolUse { id, .. } if &*id.0 == call_id))
        })
        .unwrap_or_else(|| {
            panic!(
                "{} 的历史里没有发起 {call_id} 的那条 assistant 消息",
                agent.as_str()
            )
        })
}

/// 全部轮末未读告警：`(收件人, 条数)`，按发出顺序。
///
/// 比对的是**结构化字段**而不是一段文本里 `contains` 一个 id 子串（同
/// `spawn_bg_support::orphan_warnings` 的理由）：文案改一个字这条断言不该跟着红。
pub fn unread_warnings(events: &[AgentEvent]) -> Vec<(String, usize)> {
    events
        .iter()
        .filter_map(|e| match &e.event {
            RunnerEvent::UnreadMessages { agent, count } => {
                Some((agent.as_str().to_string(), *count))
            }
            _ => None,
        })
        .collect()
}

/// 这台服务器一共被问了几次「needle 是 X」的路由——「没有新的 provider 调用发生」
/// 只能这么断，光看 `calls().len()` 会把别人的跳数也算进来。
pub fn calls_matching(server: &RoutedServer, needle: &str) -> usize {
    server.calls().iter().filter(|c| c.needle == needle).count()
}
