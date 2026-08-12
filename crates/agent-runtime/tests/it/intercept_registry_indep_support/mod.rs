//! 146 独立测试共用的一件事：造工具 spec、装注册闭包、按 call_id 读 tool_result。
//! 拆出来是因为 `intercept_registry_indep.rs`（闭包真的跑起来那三条）与
//! `intercept_registry_indep_guards.rs`（注册机制本身的边界那三条）两份文件都要
//! 用同一批名字/哨兵串/助手函数——照 `status_indep_support`/`spawn_indep_support`
//! 的既有先例，公共部分只住一处。
//!
//! 复用 `crate::support`（顶层共用假 SSE 服务器/`RunnerCtx` 装配）而不是重写：
//! `spawn_scripted_server`/`spawn_recording_server`/`sse_tool_call`/`sse_text`/
//! `build_ctx`/`build_ctx_with`/`temp_dir` 都不是本条专属的东西。
#![allow(dead_code)]

use std::sync::Arc;

use agent_core::{AgentId, ContentBlock, Session, ToolSpec};
use agent_runtime::{RunnerCtx, SessionToolFn};
use serde_json::json;

pub const TREE_TOOL: &str = "ext:test/tree_echo";
pub const WRITE_TOOL: &str = "ext:test/mark_plan";
pub const ERR_TOOL: &str = "ext:test/always_fail";
pub const GHOST_TOOL: &str = "ext:test/never-declared";
pub const UNKNOWN_TOOL: &str = "ext:test/nobody-home";

pub const TREE_SENTINEL: &str = "TREE-ECHO-SENTINEL-7f3a91";
pub const ERR_SENTINEL: &str = "ALWAYS-FAIL-SENTINEL-2b6c";

/// 一个最小合法 `ToolSpec`：schema 是空 object，够 `declares()` 判真就行——这些
/// 测试从不真的按 schema 校验入参。
pub fn spec(name: &str, description: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(description),
        schema: Arc::new(json!({ "type": "object" })),
    }
}

/// `ctx.register_session_tool` 的薄包装，只省一次 `Arc::from(name)`。
pub fn install(ctx: &mut RunnerCtx, name: &str, f: SessionToolFn) {
    ctx.register_session_tool(Arc::from(name), f);
}

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`。按 call_id
/// 取，不按顺序取——照 `status_indep_support::tool_result` 的既有先例。
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
