//! `self_indep_*` 系列测试共用的支撑代码。
//!
//! 复用 `spawn_indep_support`（按请求体路由的假服务器、`RunnerCtx` 装配、SSE
//! 脚本生成）——跟 `status_indep_support` 同一个取舍：这批用例要的服务器形状
//! （按请求体 needle 路由、一条连接一个线程）没有一处是 self 专属的，自己重写
//! 一遍不会更可信，只会多一处手误。
//!
//! 额外补一个「按 call_id 取某个 agent 的某次 tool_result」——跟
//! `status_indep_support::tool_result` 是同一份代码，这里独立一份而不是导入
//! 那边的，是为了不让 self 专题的夹具依赖 status 专题的夹具（两个专题各自的
//! 断言演进，不该因为共用一个私有帮助函数而互相牵连）。
// `unused_imports`/`dead_code` 同 `spawn_indep_support`：每个独立测试二进制各自
// `mod self_indep_support;` 引入一份拷贝，不是每个二进制都用到全部导出的东西。
#![allow(dead_code, unused_imports)]

// 有意的重复加载：`status_indep_support`/`spawn_bg_support` 都这么干，理由见
// 它们各自的模块文档——这次 CI 复活（195）不改夹具结构。
#[allow(clippy::duplicate_mod)]
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
/// 顺序断言会在多加一个调用时静默错位到隔壁那条结果上（同
/// `status_indep_support::tool_result` 的理由）。
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
