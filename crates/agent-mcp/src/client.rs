//! `McpClient`：一次握手 + 阻塞式请求-响应（issue 042 的范围——异步在飞是 043
//! 的事，这里只给一个能被 IO 线程调用的 `call(name, args) -> Result<...>`）。
//!
//! # 协议版本：协商不是断言
//!
//! `initialize` 时 client 提议 [`crate::CLIENT_PROTOCOL_VERSION`]，server 回它
//! 将采用的版本——**这里不比较两者是否相等**，只记下 server 回的版本
//! （[`McpClient::protocol_version`]）继续走下去，tools 的 list/call 形状在这几个
//! 版本间稳定（041/042 的判断）。041 探针记录的实测是 `"2025-11-25"`；042 实测
//! （真起一次 `@modelcontextprotocol/server-everything`）是 `"2025-06-18"`——
//! **跟本仓提议的版本一样**，因为该 server 在 client 提议的版本落在它支持的范围
//! 内时原样接受。版本号会随 server 升级漂移，靠协商兜底是本 crate 的显式决策，
//! 不是等它稳定下来再改常量。
//!
//! # 应答匹配要跳过 server 主动插播的通知
//!
//! 真实 server（`server-everything`）会在响应之间**插播**没有 `id` 的通知
//! （042 实测见到过 `notifications/tools/list_changed` 抢在 `tools/list` 的响应
//! 之前到达）。等响应的循环因此必须：有 `method` 字段的行（通知/server 发起的
//! 请求）跳过继续等；`id` 不匹配的响应也跳过（防御性——042 单发单收本不该出现，
//! 但跳过比殃及整次调用安全）；其余解析失败的行是真的协议错误，直接报出去，
//! 不吞（对齐 `protocol`/`jsonrpc` 两层「未知不猜成成功」的头号原则）。
//!
//! 本文件的单测（握手协商/应答匹配跳过通知/rpc error/超时/垃圾响应）挪去了
//! `tests/handshake_translate_042.rs`——红线 9：这些测试各自要 spawn 一个 `sh`
//! 假 server，字数不小，内联会顶破 300 行。那边只用这个模块的公开 API
//! （`McpClient::connect`/`list_tools`/`call`），跟 `registry.rs` 里仍然内联的
//! 单测不冲突（后者用的是下面这个 `pub(crate)` 捷径，看得到 crate 内部）。

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use agent_core::{Reversibility, ToolSpec};

use crate::error::ProtocolError;
use crate::protocol::{
    McpTool, initialize_params, parse_initialize_result, parse_tools_list, tools_call_params,
};
use crate::translate::translate;
use crate::transport::{StdioTransport, TransportError};
use crate::{RpcResponse, encode_notification, encode_request, parse_response};

/// 握手默认超时——留够 `npx` 首次拉包的时间(未缓存时要下载整个包，比常规
/// 请求慢得多)。
pub const DEFAULT_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// 普通请求(`tools/list`/`tools/call`)默认超时。
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// `McpClient` 这一层的失败：传输失败、协议解析失败、或者 server 对某次请求
/// 回了 JSON-RPC `error` 对象。
#[derive(Debug)]
pub enum McpError {
    Transport(TransportError),
    Protocol(ProtocolError),
    /// server 对某次请求回了 JSON-RPC `error` 对象。
    Rpc {
        code: i64,
        message: String,
    },
}

impl std::fmt::Display for McpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpError::Transport(e) => write!(f, "{e}"),
            McpError::Protocol(e) => write!(f, "{e}"),
            McpError::Rpc { code, message } => write!(f, "server 报错 [{code}]: {message}"),
        }
    }
}

impl std::error::Error for McpError {}

/// `list_tools` 去重时留下的痕迹：这个 server 的 `tools/list` 回包里 `tool_name`
/// 出现了不止一次，只保留了第一条——**必须点名 server id + 重复的名字 + 丢了几条**，
/// 少一样对能修它的部署方就等于没有（docs/issues/074）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateToolWarning {
    pub server_id: String,
    pub tool_name: String,
    /// 因为这个名字被丢弃的条数（不含保留的第一条）。
    pub dropped: usize,
}

impl std::fmt::Display for DuplicateToolWarning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "MCP server `{}`: tools/list 里工具名 `{}` 重复，丢弃了 {} 条（保留第一条）",
            self.server_id, self.tool_name, self.dropped
        )
    }
}

/// [`McpClient::list_tools`] 的成功返回：去重后的工具批 + 去重时留下的告警。只是
/// 给 clippy 的 `type_complexity` 让路的别名，不是新概念。
pub type ToolListOutcome = (Vec<(ToolSpec, Reversibility)>, Vec<DuplicateToolWarning>);

/// 一个已经握手成功的 MCP server 连接。持有 [`StdioTransport`](红线 3 的活
/// 句柄)——这个类型本身不 derive `Serialize`，只能住在 `McpRegistry`，不能进
/// 任何 atom(docs/MCP.md §「活句柄住 store 外」)。
pub struct McpClient {
    transport: StdioTransport,
    next_id: u64,
    /// server 在 `initialize` 响应里回的协议版本(协商结果，见模块文档)。
    pub protocol_version: String,
    pub server_name: Option<String>,
    pub capabilities: Value,
}

impl McpClient {
    /// spawn 子进程、走完握手(`initialize` → `notifications/initialized`)。
    /// 握手内部任何一步失败(起不来、超时、协议畸形)都干净返回 `Err`，不留
    /// 半握手的子进程——`StdioTransport` 的 `Drop` 会杀掉它、收尸。
    pub fn connect(
        command: &str,
        args: &[String],
        envs: &[(String, String)],
        client_name: &str,
        client_version: &str,
        handshake_timeout: Duration,
    ) -> Result<Self, McpError> {
        let transport = StdioTransport::spawn(command, args, envs).map_err(McpError::Transport)?;
        let mut client = McpClient {
            transport,
            next_id: 1,
            protocol_version: String::new(),
            server_name: None,
            capabilities: Value::Null,
        };

        let params = initialize_params(client_name, client_version);
        let result = client.request("initialize", Some(params), handshake_timeout)?;
        let parsed = parse_initialize_result(&result).map_err(McpError::Protocol)?;
        client.protocol_version = parsed.protocol_version;
        client.server_name = parsed.server_name;
        client.capabilities = parsed.capabilities;

        client.notify("notifications/initialized", None)?;
        Ok(client)
    }

    /// `tools/list` → 翻译成 `(ToolSpec, Reversibility)`(041 的 `parse_tools_list`
    /// + `translate`)。`server_id` 决定翻译出的名字前缀 `mcp:<server_id>/<tool>`。
    ///
    /// **同 server 内按工具名去重**：回包里有两项同名，只翻译第一条，后来的整条
    /// 丢弃（069 判据「只加不改」在这一跳的落地）。不这样做的后果不对称：`specs`
    /// 是 `Vec`、`push` 两次两条同名 spec 都进 prompt；`mcp_reversibility` 是
    /// `BTreeMap`、`insert` 两次是后来居上——模型看第一份说明书、undo 屏障却用
    /// 第二份的可逆性，功能照常跑，只在 `/undo` 撞上时才以错值浮出来
    /// （docs/issues/074）。丢弃了什么连同 server id、重复的名字、丢了几条一起进
    /// 第二个返回值，调用方（`loader::connect_stdio`）负责转交给能报给部署方的
    /// 那条路（跟 `Availability::Unavailable{reason}` 同一条：结构化状态，不新
    /// 发明日志）。
    pub fn list_tools(
        &mut self,
        server_id: &str,
        timeout: Duration,
    ) -> Result<ToolListOutcome, McpError> {
        let result = self.request("tools/list", Some(json!({})), timeout)?;
        let tools = parse_tools_list(&result).map_err(McpError::Protocol)?;
        Ok(dedup_by_name(&tools, server_id))
    }

    /// `tools/call`——043 的执行路由会调这个；`tool_name` 是裸名字(不带
    /// `mcp:<server>/` 前缀，见 `protocol::tools_call_params` 文档)。
    pub fn call(
        &mut self,
        tool_name: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let params = tools_call_params(tool_name, arguments);
        self.request("tools/call", Some(params), timeout)
    }

    fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;
        let bytes = encode_request(id, method, params);
        self.transport
            .write_line(&bytes)
            .map_err(McpError::Transport)?;
        self.await_response(id, timeout)
    }

    fn notify(&mut self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let bytes = encode_notification(method, params);
        self.transport
            .write_line(&bytes)
            .map_err(McpError::Transport)
    }

    /// 等一个 `id` 对上的响应，期间跳过 server 插播的通知/不对号的响应
    /// (模块文档「应答匹配」)。
    fn await_response(&mut self, id: u64, timeout: Duration) -> Result<Value, McpError> {
        let deadline = Instant::now() + timeout;
        loop {
            let line = self
                .transport
                .read_line(deadline)
                .map_err(McpError::Transport)?;
            let raw: Value = match serde_json::from_slice(line.as_bytes()) {
                Ok(v) => v,
                Err(e) => return Err(McpError::Protocol(ProtocolError::NotJson(e.to_string()))),
            };
            if raw.get("method").is_some() {
                continue; // server 主动发的通知/请求，不是我们等的响应。
            }
            match parse_response(line.as_bytes()) {
                Ok(RpcResponse::Result { id: got, result }) if got == id => return Ok(result),
                Ok(RpcResponse::Error { id: got, error }) if got == id => {
                    return Err(McpError::Rpc {
                        code: error.code,
                        message: error.message,
                    });
                }
                Ok(_) => continue, // id 不对号——防御性跳过，见模块文档。
                Err(e) => return Err(McpError::Protocol(e)),
            }
        }
    }
}

/// [`McpClient::list_tools`] 的去重实现：同一批 `tools` 里按 `McpTool.name` 判重，
/// **保留第一条、后来的整条丢弃**——`kept` 只在没见过这个名字时才 `push`，撞上的
/// 那条既不进 `kept` 也不重新 `translate`。判重只看名字，`server_id` 只用来给
/// [`DuplicateToolWarning`] 点名，不参与判重——**不做跨 server 去重**，`mcp:a/x`
/// 和 `mcp:b/x` 名字本来就不同，两次调用（各自一个 server_id）互不影响
/// （docs/MCP.md §「多来源与冲突」）。
fn dedup_by_name(
    tools: &[McpTool],
    server_id: &str,
) -> (Vec<(ToolSpec, Reversibility)>, Vec<DuplicateToolWarning>) {
    let mut kept: Vec<(ToolSpec, Reversibility)> = Vec::with_capacity(tools.len());
    let mut kept_names: Vec<&str> = Vec::with_capacity(tools.len());
    let mut warnings: Vec<DuplicateToolWarning> = Vec::new();

    for tool in tools {
        if kept_names.contains(&tool.name.as_str()) {
            match warnings.iter_mut().find(|w| w.tool_name == tool.name) {
                Some(w) => w.dropped += 1,
                None => warnings.push(DuplicateToolWarning {
                    server_id: server_id.to_string(),
                    tool_name: tool.name.clone(),
                    dropped: 1,
                }),
            }
            continue;
        }
        kept_names.push(&tool.name);
        kept.push(translate(tool, server_id));
    }

    (kept, warnings)
}

/// 用一段 `sh` 脚本假扮一个 MCP server——只回应固定脚本，零网络依赖。`registry.rs`
/// 的单测用它构造一个真的活 client(要测 insert/remove/with_client 总得先有一个)。
/// `pub(crate)`：集成测试(`tests/*.rs`)是独立编译的 crate，看不到这个符号，
/// 那边直接调 `McpClient::connect("sh", ...)`。
#[cfg(test)]
pub(crate) fn connect_fake_server(
    script: &str,
    handshake_timeout: Duration,
) -> Result<McpClient, McpError> {
    McpClient::connect(
        "sh",
        &["-c".to_string(), script.to_string()],
        &[],
        "agent-mcp-test-client",
        "0.0.0",
        handshake_timeout,
    )
}
