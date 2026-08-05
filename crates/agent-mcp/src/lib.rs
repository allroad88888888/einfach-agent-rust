//! `agent-mcp`：把一个外部 MCP server 提供的能力翻译成本仓那张扁平工具表里的项。
//!
//! 接缝的完整定义在 [docs/MCP.md](../../../docs/MCP.md)。一句话：MCP 是「外部来源差异
//! 合法存在」的地方，和 `agent-providers`（模型差异）同类，只是它要做 IO。
//!
//! # 分层（041 协议 + 042 传输/客户端/registry）
//!
//! 本 crate 分两层：
//!
//! - **协议 + 翻译**（041，`jsonrpc`/`protocol`/`translate`/`error`）：纯函数，零
//!   IO。JSON-RPC 信封、MCP 方法的 result 形状、`McpTool → (ToolSpec,
//!   Reversibility)` 的翻译。所有测试喂**录制好的字节**，不起任何进程。
//! - **stdio 传输 + 客户端 + registry**（042，`transport`/`client`/`registry`）：
//!   起子进程、newline-delimited JSON-RPC 读写、握手（`initialize` →
//!   `notifications/initialized`）、`tools/list`。这一层才引入 IO（`std::process`/
//!   `std::io`，没有新增 crate 依赖）；活句柄（`Child`/pipe/reader 线程）全部住在
//!   [`McpRegistry`]，**不进任何 atom**（红线 3，docs/MCP.md §「活句柄住 store 外」）。
//!
//! # 三样东西过接缝（进本仓）
//!
//! 1. `Vec<ToolSpec>`——喂模型的 name/description/schema（[`translate`]）。
//! 2. 每个工具的 [`agent_core::Reversibility`]——从 `readOnlyHint` 翻译（[`translate`]）。
//! 3. server 的连接/健康——042 的事。
//!
//! MCP 的 wire 类型（JSON-RPC envelope、initialize capabilities）**不过接缝**，烂在本
//! crate 里，就像 provider 的 wire 字段名烂在 `agent-providers` 里。
//!
//! # 装载（044 `.mcp.json` + 多 server + 失败隔离 + host 门）
//!
//! - **config**（`config`）：`.mcp.json` → 结构化 [`McpConfig`]。纯解析，撞名报错，远端
//!   形状只解析留位。
//! - **availability**（`availability`）：host 可用性门——[`Host`] × [`TransportKind`]。
//! - **status**（`status`）：装载后每个 server 的可序列化 [`ServerStatus`]/[`Availability`]。
//! - **loader**（`loader`）：遍历配置，spawn + 握手 + `tools/list`，失败隔离，合并工具表
//!   （[`load_servers`] → [`LoadOutcome`]）；活句柄进 [`McpRegistry`]（store 外，红线 3）。

mod availability;
mod client;
mod config;
mod error;
mod jsonrpc;
mod loader;
mod protocol;
mod registry;
mod status;
mod tool_result;
mod translate;
mod transport;

pub use availability::{Host, TransportKind};
pub use client::{
    DEFAULT_CALL_TIMEOUT, DEFAULT_HANDSHAKE_TIMEOUT, DuplicateToolWarning, McpClient, McpError,
    ToolListOutcome,
};
pub use config::{ConfigError, McpConfig, RemoteServer, ServerConfig, StdioServer, parse_config};
pub use error::ProtocolError;
pub use jsonrpc::{RpcError, RpcResponse, encode_notification, encode_request, parse_response};
pub use loader::{LoadOutcome, LoadTimeouts, load_servers};
pub use protocol::{
    Annotations, InitializeResult, McpTool, initialize_params, parse_initialize_result,
    parse_tools_list, tools_call_params,
};
pub use registry::{ClientHandle, McpRegistry};
pub use status::{Availability, ServerStatus};
pub use tool_result::{ToolCallOutput, flatten_tool_result};
pub use translate::translate;
pub use transport::TransportError;

/// 本仓 MCP client 在握手时向 server 声明的协议版本。server 在 `initialize` 响应里回一个
/// 它将采用的版本（可能不同）——真正采用哪个由 042 的握手按 server 回值定，这个常量只是
/// client 的**首选**。取值以 042 对真实 server（`@modelcontextprotocol/server-everything`）
/// 实测为准，可在 042 阶段调整。
pub const CLIENT_PROTOCOL_VERSION: &str = "2025-06-18";
