//! 041 测试场景文件共用的录制帧与小工具函数。不含任何断言——只是给
//! `tests/*_041.rs` 里各场景复用的 fixture，避免每个场景文件重新手写同一份
//! 录制帧。规格来源见各调用方文件顶部的模块文档。
//!
//! 各测试场景文件通过 `mod common;` 引入本文件（标准 Rust 集成测试共享模块
//! 写法：`tests/common/mod.rs` 不会被 cargo 当成独立测试目标）。

// 每个 `tests/*.rs` 独立编译成一个 crate，只 `use` 本文件里自己那个场景需要的
// helper——没用到的那个二进制里就报 `dead_code`。但这些 helper 明明被别的场景
// 文件用着（如 `everything_tools_list_frame` 被 tools_list_041 用），是 Rust 集成
// 测试共享 `common/mod.rs` 的结构性假阳性，不是真死代码。整份文件放行。
#![allow(dead_code)]

use agent_mcp::{Annotations, McpTool, RpcResponse, parse_response};
use serde_json::Value;

/// 把一条"录制的"响应字节先过 JSON-RPC 信封（`parse_response`），再取出
/// `result`，喂给 `parse_initialize_result` / `parse_tools_list`。模拟真实
/// 调用链：先信封后方法。
pub fn recorded_result(bytes: &[u8]) -> Value {
    match parse_response(bytes).expect("录制帧应当是合法 JSON-RPC 响应") {
        RpcResponse::Result { result, .. } => result,
        RpcResponse::Error { error, .. } => panic!("录制帧不该是 error 响应: {error:?}"),
    }
}

/// 录制的 `tools/list` 响应：4 个工具，name 照抄
/// `@modelcontextprotocol/server-everything`，顺序固定为
/// echo / add / printEnv / sendEmail（用来断言 `parse_tools_list` 保序）。
pub fn everything_tools_list_frame() -> &'static [u8] {
    br#"{"jsonrpc":"2.0","id":2,"result":{"tools":[
        {"name":"echo","description":"Echoes back the input","inputSchema":{"type":"object","properties":{"message":{"type":"string"}},"required":["message"]},"annotations":{"readOnlyHint":true,"title":"Echo Tool"}},
        {"name":"add","description":"Adds two numbers","inputSchema":{"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]},"annotations":{"readOnlyHint":true}},
        {"name":"printEnv","description":"Prints environment variables","inputSchema":{"type":"object","properties":{}}},
        {"name":"sendEmail","description":"Sends an email","inputSchema":{"type":"object","properties":{"to":{"type":"string"}},"required":["to"]},"annotations":{"readOnlyHint":false,"destructiveHint":true}}
    ]}}"#
}

/// 直接构造一个 `McpTool`（绕开 `parse_tools_list`），隔离 `translate()` 本身
/// 的可逆性翻译逻辑，不与 parse 层的 bug 混在一起断言。
pub fn tool_with_annotations(name: &str, annotations: Option<Annotations>) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: Some("d".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
        annotations,
    }
}
