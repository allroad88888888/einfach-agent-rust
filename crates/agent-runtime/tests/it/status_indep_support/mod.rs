//! 051 端到端用例的共用件：029 那份假服务器/装配夹具**原样复用**，外加三个
//! 「怎么读一段 status 正文」的断言助手。
//!
//! 复用而不是重写：`spawn_indep_support` 的内容（按请求体路由的并发假服务器、
//! `RunnerCtx` 装配、SSE 脚本生成）没有一处是 spawn 专属的，而这三个用例要的
//! 正是同一种服务器形状（一条连接一个线程，子 agent 的请求真的并行到达）。
//! 自己再抄一份不会更可信，只会多一处手误。
#![allow(dead_code, unused_imports)]

#[path = "../spawn_indep_support/mod.rs"]
pub mod base;

pub use base::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir,
    wire_tool_name,
};

use agent_core::{AgentId, ContentBlock, Session};

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`。
///
/// **按 call_id 取，不按顺序取**：一条 assistant 消息里可以并列好几个调用，
/// 顺序断言会在多加一个调用时静默错位到隔壁那条结果上。
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

/// status 正文里每一行开头那个 agent id（跳过标题行）。
///
/// **不能用 `body.contains("root/a1")` 断言**：`root/a1` 是 `root/a1/a1` 的子串，
/// 在 agent 树上那是个假绿灯——「兄弟不该出现」这类断言必须逐行取第一个字段比
/// 集合，否则红线 10 破了测试还是绿的。
pub fn listed_ids(body: &str) -> Vec<&str> {
    body.lines()
        .skip(1)
        .map(|line| line.split(' ').next().unwrap())
        .collect()
}

/// status 正文里每一行的 activity 字段（`id depth=N <activity> task=...` 的第三段）。
pub fn listed_activities(body: &str) -> Vec<&str> {
    body.lines()
        .skip(1)
        .map(|line| line.split(' ').nth(2).unwrap())
        .collect()
}
