//! 029 独立测试共用的支撑代码。
//!
//! 复用实现方新增的按请求体路由假服务器 `tests/support/routed.rs`——
//! 029 的任务说明明确点名它是「夹具不是实现」，可以复用：一条连接一个
//! 线程、按请求体 needle 路由，这正是并行子 agent 断言「到达时间窗重叠」
//! 需要的服务器形状，自己重写一遍不会更可信，只会更容易出手误。
//!
//! `RunnerCtx` 的装配**不**借实现方 `tests/support/mod.rs::build_ctx_with`
//! 的代码：这份独立测试对 `RunnerCtx` 的理解只来自 029 rustdoc 面
//! （`RunnerCtx::new` + `with_agent_events`，见 crates/agent-runtime/src/
//! ctx.rs 的 pub 签名），构造参数照抄的是同一份公开契约，不是抄实现。
// `unused_imports` 一起放行：每个独立测试二进制各自 `mod spawn_indep_support;`
// 引入本文件一份拷贝，不是每个二进制都用到全部导出的类型/函数——跟实现方
// `tests/support/mod.rs` 顶部同一个取舍（比如 `spawn_indep_cancel_tree.rs`
// 自己写了最小服务器，不需要这里的 `Route`/`RoutedServer` 重导出）。
#![allow(dead_code, unused_imports)]

#[path = "../support/routed.rs"]
pub mod routed;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use agent_core::SessionConfig;
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{AgentEvent, RunnerCtx, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

pub use routed::{Route, RoutedServer};

/// wire 上的工具名转义。三家 provider 是 OpenAI 兼容协议，非字母数字/下划线的
/// 字符一律转成 `_` + 两位大写十六进制（`probes/results/wire-shape.json` 与
/// `agent-providers` 的録制帧统一遵守这条规则：`srv:fs/read` →
/// `srv_3Afs_2Fread`，见 `spawn_wire_name_matches_the_known_escape` 那条自检）。
/// 独立测试脚本 SSE 帧要自己拼工具名，这个映射是编造响应的必需品。
pub fn wire_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c.to_string()
            } else {
                format!("_{:02X}", c as u32)
            }
        })
        .collect()
}

/// `srv:agent/spawn` 的 wire 形式，脚本里反复要用，焊成常量避免每处重算。
pub const SPAWN_WIRE: &str = "srv_3Aagent_2Fspawn";

/// 每个用例一个独立临时目录，不清理（跟实现方 `tests/support/mod.rs` 同一个
/// 取舍：OS/CI 环境自行回收）。
pub fn temp_dir(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("spawn-indep-{name}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 装一个指向本地假服务器的 `RunnerCtx`，事件回调带 agent 归属
/// （`with_agent_events`，029 的多 agent 形状）。provider 用 DeepSeek——
/// 跟实现方同一家，三家里已经有録制帧验证过 wire 形状的那家。
pub fn build_ctx(
    port: u16,
    root: &std::path::Path,
    tools: ToolTable,
) -> (RunnerCtx, Rc<RefCell<Vec<AgentEvent>>>) {
    let client = Client::with_config(
        Duration::from_secs(5),
        Duration::from_millis(50),
        Backoff {
            base: Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let fs = ToolExecutor::new(root).unwrap();
    let session_config = SessionConfig {
        model: Arc::from("deepseek-v4-pro"),
        temperature: None,
        max_tokens: None,
        context_window: None,
    };

    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        tools,
        Vec::new(),
        session_config,
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_| {}),
    );
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);
    let ctx = ctx.with_agent_events(Box::new(move |ev| sink.borrow_mut().push(ev)));
    (ctx, events)
}

/// 一次 SSE 帧：单个 `tool_calls` 增量（起手 id/name 齐了，参数一次给全，不
/// 分片）+ 收尾帧。够用——独立测试关心的是泵的编排行为，不是流式累加器本身
/// （那是 agent-providers 自己的测试范围）。`input_json` 是给工具的入参
/// **原始 JSON 文本**（如 `r#"{"task":"..."}"#`），`arguments` 字段本身是一个
/// 装着这段 JSON 的**字符串**（wire 上的既有形状，见 `happy_two_hop.rs` 録制
/// 帧），用 `json!` 序列化而不是手拼大括号，转义交给库，不留手误的空间。
pub fn sse_tool_call(call_id: &str, wire_tool: &str, input_json: &str) -> Vec<String> {
    let chunk1 = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": { "name": wire_tool, "arguments": input_json }
                }]
            },
            "finish_reason": null
        }]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}

/// 两个并行 `tool_calls`（`index` 0/1）在同一帧里一次给全，收尾帧
/// `finish_reason: "tool_calls"`——给三子并行测试的 root 首跳用（一次说三个
/// 子，用两次调用拼：先两个一起发，第三个走 `sse_tool_call` 风格另起一路也
/// 行，但同一帧更接近真实 provider 一次性吐出并行 tool_calls 的形状，见
/// `probes/results/wire-shape.json` 的 `parallel.tool_calls`）。
pub fn sse_tool_calls(calls: &[(&str, &str, &str)]) -> Vec<String> {
    let tool_calls: Vec<_> = calls
        .iter()
        .enumerate()
        .map(|(i, (call_id, wire_tool, input_json))| {
            serde_json::json!({
                "index": i,
                "id": call_id,
                "type": "function",
                "function": { "name": wire_tool, "arguments": input_json }
            })
        })
        .collect();
    let chunk1 = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {"role": "assistant", "content": null, "tool_calls": tool_calls},
            "finish_reason": null
        }]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}

/// 一次 SSE 帧：纯文本收尾（`StopReason::EndTurn`）。
pub fn sse_text(text: &str) -> Vec<String> {
    let chunk1 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "stop"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_wire_name_matches_the_known_escape() {
        assert_eq!(wire_tool_name(agent_runtime::SPAWN_TOOL), SPAWN_WIRE);
        // 对照 probes/results 与 agent-providers 録制帧里已验证过的既有映射，
        // 不是这份测试自己现造的假设。
        assert_eq!(wire_tool_name("srv:fs/read"), "srv_3Afs_2Fread");
    }

    /// 自检：脚本生成的每一行都是合法 JSON（去掉 "data: " 前缀之后），且
    /// `arguments` 字段真的是一个装着 `input_json` 原文的字符串——这份测试
    /// 全靠这几个生成函数造出可信的假响应，生成函数自己先被验一遍。
    #[test]
    fn sse_tool_call_lines_are_valid_json_with_stringified_arguments() {
        let lines = sse_tool_call("call_x", "srv_3Aagent_2Fspawn", r#"{"task":"do it"}"#);
        assert_eq!(lines.len(), 3);
        let first = lines[0].strip_prefix("data: ").unwrap();
        let v: serde_json::Value = serde_json::from_str(first).unwrap();
        let args = v["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"]
            .as_str()
            .unwrap();
        assert_eq!(args, r#"{"task":"do it"}"#);
        assert_eq!(lines[2], "data: [DONE]");
    }

    #[test]
    fn sse_text_line_round_trips_the_exact_text() {
        let lines = sse_text("hello \"world\"");
        let first = lines[0].strip_prefix("data: ").unwrap();
        let v: serde_json::Value = serde_json::from_str(first).unwrap();
        assert_eq!(
            v["choices"][0]["delta"]["content"].as_str().unwrap(),
            "hello \"world\""
        );
    }
}
