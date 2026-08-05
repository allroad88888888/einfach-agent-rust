//! 043 的 MCP 集成测试共用件：起一个 `sh` 脚本假 server 连成 client 装进 registry，
//! 翻译一个工具喂 `ToolTable::with_mcp`，再拼几段 DeepSeek wire 的脚本响应。
//!
//! 假 server 是一段 `sh`（零网络、零 npm），照 `agent-mcp` 自己的假 server 手法：
//! `read` 逐行吃请求、`printf` 逐行回响应。client 连接时的 id 序列是确定的
//! （`initialize`=1，之后每个请求 +1；`notifications/initialized` 是通知不占 id），
//! 所以脚本里 `tools/call` 的响应 id 写死为 `2`——只要测试不在中间插 `list_tools`。

#![allow(dead_code)]

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{Reversibility, ToolSpec};
use agent_mcp::{translate, Annotations, McpClient, McpRegistry, McpTool};
use agent_runtime::{AgentEvent, RunnerCtx, ToolTable};
use serde_json::json;

use super::ScriptedResponse;

/// 装一份接了 MCP 的 agent-aware `RunnerCtx`：内置只读集 + `entries` 里的 MCP 工具，
/// registry 里是 `server`/`script` 那个假 server，MCP 往返超时压到 5s（测试要快）。
pub fn build_ctx(
    port: u16,
    dir: &Path,
    server: &str,
    entries: Vec<(ToolSpec, Reversibility)>,
    script: &str,
) -> (RunnerCtx, Rc<RefCell<Vec<AgentEvent>>>) {
    let table = ToolTable::builtin().with_mcp(entries);
    let (ctx, events) = super::build_ctx_agent_aware(port, dir, table);
    let registry = registry_with_fake_server(server, script);
    let ctx = ctx
        .with_mcp(registry)
        .with_mcp_timeout(Duration::from_secs(5));
    (ctx, events)
}

/// 起一个 `sh` 脚本假 server、握手成功、装进一张新 registry。`server_id` 决定
/// `mcp:<server_id>/<tool>` 命名。
pub fn registry_with_fake_server(server_id: &str, script: &str) -> Arc<McpRegistry> {
    let client = McpClient::connect(
        "sh",
        &["-c".to_string(), script.to_string()],
        &[],
        "agent-runtime-mcp-test",
        "0.0.0",
        Duration::from_secs(5),
    )
    .expect("假 MCP server 该握手成功");
    let registry = Arc::new(McpRegistry::new());
    registry.insert(server_id.to_string(), client);
    registry
}

/// 翻译一个 MCP 工具成 `(ToolSpec, Reversibility)`（喂 `ToolTable::with_mcp`）。
/// `read_only=true` → `Pure` 无屏障；`false` → `Irreversible` 带屏障。
pub fn tool_entry(server_id: &str, name: &str, read_only: bool) -> (ToolSpec, Reversibility) {
    let tool = McpTool {
        name: name.to_string(),
        description: Some(format!("{name} tool")),
        input_schema: json!({"type": "object", "properties": {"message": {"type": "string"}}}),
        annotations: Some(Annotations {
            read_only_hint: Some(read_only),
        }),
    };
    translate(&tool, server_id)
}

/// 一段 sh 脚本：握手 → 读 `tools/call` → `sleep` 一会 → 回一条结果。`sleep_secs`
/// 让「慢响应」可控（红线 6 的对抗测试要在响应回来之前 bump epoch）。`result` 是
/// `tools/call` 的 result 对象（`{content:[...],isError?:...}`）的 JSON 文本。
pub fn call_script(sleep_secs: &str, result: &str) -> String {
    format!(
        r#"read init
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}}}}}}'
read initialized
read call
sleep {sleep_secs}
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{result}}}'
sleep 5
"#
    )
}

/// 一段 sh 脚本：握手 → 读 `tools/call` → 回一条 JSON-RPC `error`（server 侧失败）。
pub fn call_error_script(code: i64, message: &str) -> String {
    format!(
        r#"read init
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}}}}}}'
read initialized
read call
printf '%s\n' '{{"jsonrpc":"2.0","id":2,"error":{{"code":{code},"message":"{message}"}}}}'
sleep 5
"#
    )
}

/// hop1：DeepSeek wire 的一条工具调用响应。`wire_name` 是转义后的工具名
/// （`mcp:slow/echo` → `mcp_3Aslow_2Fecho`，见 agent-providers wire::names）。
pub fn hop_tool_use(wire_name: &str, call_id: &str) -> ScriptedResponse {
    super::sse_tool_call(call_id, wire_name, r#"{\"message\": \"hi\"}"#)
}

/// hop2：一条普通 `EndTurn` 文本响应（工具结果回来之后模型收敛）。
pub fn hop_end_turn() -> ScriptedResponse {
    super::sse_text("收到了")
}
