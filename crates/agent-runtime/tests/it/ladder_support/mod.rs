//! 108（阶梯编排）系列测试共用的脚手架。
//!
//! `crate::support::build_ctx*` 家族把 `SessionConfig.context_window` 焊死成
//! `None`——102/103 那批测试不需要真的触发阶梯，103 甚至是手动摆 `SendPlan`
//! 模拟触发之后的样子。108 恰恰要让**自动阶梯真的开火**，所以这里另起一个
//! ctx 构造器，把 `context_window` 开成调用方可控的参数；`RunnerCtx` 本身没有
//! 「起飞之后再改 `context_window`」的口子（那是起飞时固化的 `SessionConfig`，
//! 不是运行期可变状态），只能在构造时给对。
//!
//! 其余部件（假服务器、`temp_dir`）直接复用 `crate::support` / `crate::support::routed`
//! ——不重新发明轮子，本文件只补两样 108 独有的东西：
//! 1. `build_ctx`：`context_window` 可控的 ctx。
//! 2. `text_response` / `tool_call_response`：`usage.prompt_tokens` 可控的 SSE 帧——
//!    108 的阶梯只看这个数字做算术（096 决策记录：触发是纯算术，不量真实
//!    token），跟请求体本身多大无关，所以测试要能自己钦定这个数字。

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use agent_core::SessionConfig;
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{AgentEvent, RunnerCtx, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::{Backoff, Client};

/// 压缩子 agent 固定提示词的一段（`agent_runtime::compact_spawn` 的
/// `SUMMARY_INSTRUCTIONS` 开头那句）——测试拿它在假服务器上认出「这条请求是
/// 压缩子在说话」，也拿它断言「父的下一轮请求里不该出现这句话」（108 验收：
/// 父的 encode 不含子 agent 的摘要过程）。字面量必须跟实现那边逐字一致，否则
/// 路由匹配不上、测试会挂在读空连接上——这条耦合是故意的，压缩子的指令是
/// 108/106 定死的产品文案，不是随时可能改的实现细节。
pub const SUMMARY_PROMPT_NEEDLE: &str = "把下面这段对话历史压缩成一份摘要";

/// 装一个指向本地假服务器的 `RunnerCtx`，`context_window` 由调用方定。
///
/// 事件用 [`AgentEvent`]（带归属）收集，不是裸 `RunnerEvent`——108 的测试要分清
/// 一条 `Notice::CompactionSummaryReceived` 是说给 root 听的还是说给压缩子听的
/// （`Notice` 自己不带 `agent` 字段，归属全靠 `AgentEvent.agent`，见
/// `engine/notice.rs` 模块文档）。
pub fn build_ctx(
    port: u16,
    root: &Path,
    tools: ToolTable,
    context_window: Option<u32>,
) -> (RunnerCtx, Rc<RefCell<Vec<AgentEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&events);

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
        context_window,
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
        Box::new(|_ev| {}),
    )
    .with_agent_events(Box::new(move |ev| sink.borrow_mut().push(ev)));
    (ctx, events)
}

/// DeepSeek wire：纯文本收尾（`StopReason::EndTurn`），`prompt_tokens` 由调用方定。
pub fn text_response(text: &str, prompt_tokens: u32) -> Vec<String> {
    let chunk1 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "stop"}],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": 5,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": prompt_tokens
        }
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}

/// DeepSeek wire：一条工具调用（hop1），`prompt_tokens` 由调用方定。
pub fn tool_call_response(
    call_id: &str,
    wire_tool: &str,
    input_json: &str,
    prompt_tokens: u32,
) -> Vec<String> {
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
                    "function": {"name": wire_tool, "arguments": input_json}
                }]
            },
            "finish_reason": null
        }]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "tool_calls"}],
        "usage": {
            "prompt_tokens": prompt_tokens,
            "completion_tokens": 5,
            "prompt_cache_hit_tokens": 0,
            "prompt_cache_miss_tokens": prompt_tokens
        }
    });
    vec![
        format!("data: {chunk1}"),
        format!("data: {chunk2}"),
        "data: [DONE]".to_string(),
    ]
}
