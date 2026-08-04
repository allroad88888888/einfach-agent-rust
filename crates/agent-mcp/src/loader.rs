//! 多 server 装载 + 失败隔离。**这一件事**：遍历一份 [`McpConfig`]，逐个 spawn +
//! 握手 + `tools/list`，把所有工具汇进一张表、可用性汇进一份结构化状态，活句柄塞进
//! [`McpRegistry`]（store 外，红线 3）。
//!
//! # 失败隔离（产品判断）
//!
//! 一个 server 命令不存在 / 握手失败 / 超时 / `tools/list` 失败 → 标
//! [`Availability::Unavailable`]（带原因），**其余照常连、会话照常起**。不是「一个坏
//! 了全崩」——对齐 Claude Code 的 `/mcp`：failed server 不阻塞会话。错误进结构化状态，
//! 不 panic、不吞（docs/MCP.md §失败隔离）。
//!
//! # 顺序 = 配置顺序（红线 11）
//!
//! 合并出的工具表按 server 在配置里的顺序拼接，server 内按 `tools/list` 顺序——两级都
//! 确定，进 prompt 的东西不漂。撞名在 [`parse_config`](crate::parse_config) 那层就报了，
//! 到这里 server id 已保证唯一，不会静默覆盖 registry。
//!
//! # 同 server 内的重复工具名（issue 074）
//!
//! `parse_config` 拦的是**跨 server 的 id 撞名**；一个 server 自己的 `tools/list`
//! 回包里两项同名是另一类问题，靠 [`McpClient::list_tools`] 在翻译那一跳去重
//! （保留第一条，丢后来的整条）。去重不阻断连接——server 照常标 `Connected`，
//! 丢弃的痕迹（server id + 重复的名字 + 丢了几条）汇进 [`LoadOutcome::warnings`]，
//! 跟 `Availability::Unavailable{reason}` 同一条「结构化状态，不新发明日志」的路。

use std::time::Duration;

use agent_core::{Reversibility, ToolSpec};

use crate::availability::Host;
use crate::client::{DEFAULT_CALL_TIMEOUT, DEFAULT_HANDSHAKE_TIMEOUT, McpClient};
use crate::config::{McpConfig, ServerConfig, StdioServer};
use crate::registry::McpRegistry;
use crate::status::{Availability, ServerStatus};

/// 装载时的超时预算。握手默认留够 `npx` 首次拉包的时间；普通请求短些。
#[derive(Clone, Copy, Debug)]
pub struct LoadTimeouts {
    pub handshake: Duration,
    pub call: Duration,
}

impl Default for LoadTimeouts {
    fn default() -> Self {
        Self { handshake: DEFAULT_HANDSHAKE_TIMEOUT, call: DEFAULT_CALL_TIMEOUT }
    }
}

/// 一次装载的产物：合并后的工具表 + 每个 server 的可用性状态 + 装载期发现的告警。
///
/// `tools` 喂 `ToolTable`（045 接线），`servers` 是「谁连上了谁没有 + 原因」的可序列化
/// 报告（`/mcp` 状态命令用）。活句柄不在这里——它们进了传入的 `McpRegistry`。
#[derive(Debug)]
pub struct LoadOutcome {
    /// 所有连上的 server 的工具，按配置顺序合并。名字带 `mcp:<id>/` 前缀，天然消歧
    /// 两个 server 的同名工具（`mcp:a/x` vs `mcp:b/x` 不撞）。
    pub tools: Vec<(ToolSpec, Reversibility)>,
    /// 每个 server 一条状态，顺序 = 配置顺序。
    pub servers: Vec<ServerStatus>,
    /// **不阻断连接**的告警文案，按发现顺序排列。目前唯一的来源是
    /// `McpClient::list_tools` 去重同 server 内重复工具名时留下的痕迹
    /// （模块文档「同 server 内的重复工具名」）——每条都含 server id、重复的
    /// 工具名、丢了几条（[`crate::client::DuplicateToolWarning`] 的 `Display`）。
    pub warnings: Vec<String>,
}

impl LoadOutcome {
    /// 连上的 server id（`/mcp` 与诊断用）。
    pub fn connected_ids(&self) -> Vec<&str> {
        self.servers.iter().filter(|s| s.is_connected()).map(|s| s.id.as_str()).collect()
    }

    /// 每个 server 的状态——「谁连上了、谁没有、为什么」。等价于验收里说的
    /// `available_servers()`。
    pub fn available_servers(&self) -> &[ServerStatus] {
        &self.servers
    }
}

/// 装载一份配置里的所有 server。**永不整体失败**——单个 server 的问题落进它自己的
/// `ServerStatus`，返回值总是完整的（配置本身的解析错在 `parse_config` 那层就拦了）。
///
/// `host` 走可用性门：M6 的 CLI 传 [`Host::Server`]，stdio 恒可用。浏览器 host + stdio
/// 会走到「host 不支持」分支（形状留位，M6 不产生这种配置）。`client_name`/
/// `client_version` 是握手时本仓 client 的自报身份。
pub fn load_servers(
    config: &McpConfig,
    registry: &McpRegistry,
    host: Host,
    timeouts: LoadTimeouts,
    client_name: &str,
    client_version: &str,
) -> LoadOutcome {
    let mut tools: Vec<(ToolSpec, Reversibility)> = Vec::new();
    let mut servers: Vec<ServerStatus> = Vec::with_capacity(config.servers.len());
    let mut warnings: Vec<String> = Vec::new();

    for (id, cfg) in &config.servers {
        let availability = load_one(
            id,
            cfg,
            host,
            registry,
            &timeouts,
            client_name,
            client_version,
            &mut tools,
            &mut warnings,
        );
        servers.push(ServerStatus { id: id.clone(), availability });
    }

    LoadOutcome { tools, servers, warnings }
}

/// 装载单个 server。远端 → 暂不支持；host 门不通过 → 不可用；stdio 且门通过 → 真连。
#[allow(clippy::too_many_arguments)]
fn load_one(
    id: &str,
    cfg: &ServerConfig,
    host: Host,
    registry: &McpRegistry,
    timeouts: &LoadTimeouts,
    client_name: &str,
    client_version: &str,
    tools: &mut Vec<(ToolSpec, Reversibility)>,
    warnings: &mut Vec<String>,
) -> Availability {
    let stdio = match cfg {
        ServerConfig::Remote(r) => {
            return Availability::Unsupported {
                reason: format!(
                    "远端传输 `{}` 在 M6 未实现（配置已解析，形状留位，等 http 传输的延后 issue）",
                    r.transport_type
                ),
            };
        }
        ServerConfig::Stdio(s) => s,
    };

    if !cfg.available_on(host) {
        // M6 的 CLI 是 server host，走不到这里；浏览器 host + stdio 才命中（形状留位）。
        return Availability::Unavailable {
            reason: format!("stdio 传输在 {} host 上不可用（没有子进程）", host.label()),
        };
    }

    connect_stdio(id, stdio, registry, timeouts, client_name, client_version, tools, warnings)
}

/// 真连一个 stdio server：spawn + 握手 + `tools/list`。任何一步失败都干净落
/// `Unavailable`——client 若已握手成功但 `tools/list` 失败，函数返回时 `client` drop，
/// `StdioTransport::Drop` 杀掉子进程收尸，不把半连的 client 塞进 registry。
#[allow(clippy::too_many_arguments)]
fn connect_stdio(
    id: &str,
    s: &StdioServer,
    registry: &McpRegistry,
    timeouts: &LoadTimeouts,
    client_name: &str,
    client_version: &str,
    tools: &mut Vec<(ToolSpec, Reversibility)>,
    warnings: &mut Vec<String>,
) -> Availability {
    let envs = s.env_pairs();
    let mut client = match McpClient::connect(
        &s.command,
        &s.args,
        &envs,
        client_name,
        client_version,
        timeouts.handshake,
    ) {
        Ok(c) => c,
        Err(e) => return Availability::Unavailable { reason: format!("连接失败: {e}") },
    };

    let (batch, dup_warnings) = match client.list_tools(id, timeouts.call) {
        Ok(v) => v,
        Err(e) => return Availability::Unavailable { reason: format!("tools/list 失败: {e}") },
    };

    let tool_count = batch.len();
    tools.extend(batch);
    // 重复工具名不阻断连接（模块文档「同 server 内的重复工具名」）——server 照常
    // Connected，痕迹汇进 warnings，跟 Unavailable{reason} 同一条结构化状态的路。
    warnings.extend(dup_warnings.iter().map(ToString::to_string));
    registry.insert(id.to_string(), client);
    Availability::Connected { tool_count }
}
