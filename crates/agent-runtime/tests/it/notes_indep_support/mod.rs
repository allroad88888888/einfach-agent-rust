//! 209 `notes_indep_*` 系列共用的支撑代码。
//!
//! 复用 `spawn_indep_support`（按请求体路由的假服务器、`RunnerCtx` 装配、SSE 脚本
//! 生成）——跟 `self_indep_support`/`status_indep_support` 同一个取舍：这批用例
//! 要的服务器形状没有一处是 notes 专属的。**故意不 `use` 那两个专题现成的
//! `notes_indep_support`（不存在）或 `self_indep_support`**：209 与「另一个正在
//! 同时落地的独立测试 agent」各自往 `tests/it/` 写文件、各自改一次 `main.rs`
//! 那一行 `mod`，两份互不依赖才不会因为对方还没写完而编译不过。
#![allow(dead_code, unused_imports)]

#[allow(clippy::duplicate_mod)]
#[path = "../spawn_indep_support/mod.rs"]
pub mod base;

pub use base::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir,
    wire_tool_name,
};

use agent_core::{AgentId, ContentBlock, Session};

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`。
/// 按 call_id 取，不按顺序取——理由同 `self_indep_support::tool_result`。
pub fn tool_result(session: &Session, agent: &AgentId, call_id: &str) -> (String, bool) {
    let messages = session.messages_of(agent);
    messages
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
                "{} 的历史里没有 call_id={call_id} 的 tool_result：{messages:#?}",
                agent.as_str()
            )
        })
}
