//! 212 独立测试的共用件：复用 `spawn_indep_support` 的并发假服务器 / `RunnerCtx`
//! 装配（跟 `send_indep_support` 同一条理由），外加几个「按 call_id 取一次
//! `srv:agent/await` 的结果」的断言助手。
//!
//! # 这份测试的黑盒来源
//!
//! `docs/issues/212-await-tool-and-wait-graph.md`（规格本体）、
//! `agent_core` / `agent_runtime` **导出面上的签名与 rustdoc**（`AWAIT_TOOL` /
//! `await_spec` / `ToolTable::with_await` / `Session::await_agent` /
//! `Session::awaiting_on` / `Session::await_progress` 等）、以及
//! `docs/INVARIANTS.md` 红线 1 / 6 / 10 / 11。
//!
//! **没有读**：`crates/agent-core/src/command/awaiting.rs`、
//! `crates/agent-core/src/graph/build.rs`、
//! `crates/agent-runtime/src/await_tool.rs`、
//! `crates/agent-runtime/src/await_slot.rs`——这四个文件全程没打开
//! （WORKFLOW §三：看了实现，测的就只剩实现想到的那几条路径）。
#![allow(dead_code, unused_imports)]

// 同 `send_indep_support` / `spawn_bg_support` 那条：有意的重复加载，去重会把
// 几处的 `RoutedServer` 合成同一个类型身份，那是夹具结构的改动，不在这次任务里做。
#[allow(clippy::duplicate_mod)]
#[path = "../spawn_indep_support/mod.rs"]
pub mod base;

pub use base::{
    Route, RoutedServer, build_ctx, sse_text, sse_tool_call, sse_tool_calls, temp_dir,
    wire_tool_name,
};

use agent_core::{AgentId, ContentBlock, Session};

/// `srv:agent/await` 的 wire 形式，脚本里反复要用。跟 `SEND_WIRE`/`SPAWN_WIRE`
/// 同一条既有转义规则（`spawn_wire_name_matches_the_known_escape` 那条自检覆盖
/// 的是同一个映射），下面的单元测试再钉一次这个具体值。
pub const AWAIT_WIRE: &str = "srv_3Aagent_2Fawait";

/// 某个 agent 历史里 `call_id` 那一次调用的结果：`(正文, is_error)`。
///
/// **按 call_id 取，不按顺序取**——跟 `send_indep_support::tool_result` 同一条
/// 理由：一条 assistant 消息里可能并列好几个调用，顺序断言会在多加一个调用时
/// 静默错位到隔壁那条结果上。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 自检：`AWAIT_WIRE` 这个手拼常量跟 `wire_tool_name` 真的算出来的值一致
    /// ——脚本里到处要用这个字符串，先把它钉死，用错了在这里先红。
    #[test]
    fn await_wire_name_matches_the_known_escape() {
        assert_eq!(wire_tool_name(agent_runtime::AWAIT_TOOL), AWAIT_WIRE);
    }
}
