//! `.mcp.json` 解析：`mcpServers` 对象 → 结构化配置。**只管把本地配置文件读成
//! 类型，不做任何 IO 连接**（连接/握手是 `loader` 的事）。跟 Claude Code 的
//! `.mcp.json` 对齐（key = server id，进 `mcp:<id>/<tool>` 命名）。
//!
//! # 信任级：本地文件，不是网络可控（红线 8 邻近）
//!
//! `command`/`args`/`env` 会被 `loader` 拿去 spawn 外部进程——和 `providers.toml`
//! 同信任级（本地文件）。本模块只从**本地路径**读（[`McpConfig::from_file`］）或从
//! 内存字符串解析（[`parse_config`]），不存在从网络请求体喂进来的路径，别把它接成
//! 网络可控的输入。
//!
//! # 两种形状
//!
//! - **stdio**（M6 实做）：`{command, args, env}`。
//! - **远端**（M6 只解析、不装载）：`{type: "http"|"sse", url, headers}`。远端形状
//!   出现在配置里**不该让整个文件解析失败**——它被解析成 [`ServerConfig::Remote`]，
//!   由 `loader` 标「暂不支持」（形状留位，等 http 传输的延后 issue）。
//!
//! # 撞名是配置错误，不静默取后者
//!
//! 同一个 server id 在 `mcpServers` 里出现两次（重复的 JSON key）→
//! [`ConfigError::DuplicateServerId`]。解析全程**保序、保留重复**（流式 `visit_map`
//! 逐条收，不经 `serde_json::Value` 的 `Map`——那个会静默取后者），撞名在 Rust 层
//! 显式报出来（docs/MCP.md §「多来源与冲突」）。

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use serde::de::{self, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::availability::{Host, TransportKind};

/// 一份 `.mcp.json` 解析后的结果：按**配置里出现的顺序**排列的 `(server id, 配置)`。
/// 顺序保留是红线 11 的一部分——`loader` 按这个顺序合并工具表，进 prompt 的东西
/// 顺序不能漂。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct McpConfig {
    pub servers: Vec<(String, ServerConfig)>,
}

/// 一个 server 的配置。stdio 是 M6 实做的形状；远端（http/sse）只解析、留位。
///
/// 这个类型（连同下面的 `StdioServer`/`RemoteServer`）**可序列化**——server 的逻辑
/// 标识与命令行会进 atom/快照（红线 3：活句柄住 store 外，配置进 store）。注意它的
/// `Serialize`/`Deserialize` 是**内部快照格式**（externally-tagged），跟 `.mcp.json`
/// 的 wire 形状不是一回事：`.mcp.json` 走 [`parse_config`] 的分类逻辑，不走 derive。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerConfig {
    Stdio(StdioServer),
    /// 远端形状，M6 不实现传输，`loader` 标「暂不支持」。
    Remote(RemoteServer),
}

/// stdio server：spawn `command`，带 `args`，环境变量**追加**在继承的父环境之上
/// （见 `transport::StdioTransport::spawn`）。`env` 用 `BTreeMap` 存——有序、可序列化
/// （红线 11 不许 `HashMap`），spawn 时转成有序的 `Vec<(String,String)>`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StdioServer {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
}

impl StdioServer {
    /// `env` 摊平成 `transport::spawn` 要的有序键值对（`BTreeMap` 迭代即字典序，
    /// 确定）。
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        self.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// 远端 server 的**留位形状**（M6 不实现传输）。字段解析进来只为诊断/`/mcp` 展示与
/// 未来的 http 传输，M6 不读 `url`/`headers` 去连。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RemoteServer {
    /// `"http"` / `"sse"`（或其它未识别的非 stdio type，一律当远端留位）。
    pub transport_type: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
}

impl ServerConfig {
    /// 这个 server 需要的传输种类——host 可用性门（[`ServerConfig::available_on`]）用它。
    pub fn transport_kind(&self) -> TransportKind {
        match self {
            ServerConfig::Stdio(_) => TransportKind::Stdio,
            ServerConfig::Remote(_) => TransportKind::Remote,
        }
    }

    /// host 可用性门：这个源在给定 host 上跑得起来吗（docs/MCP.md §「host 能力差异」）。
    /// stdio 只有 server/桌面 host 有，浏览器 host 只能远端——门在这里表达，不假装
    /// stdio server 在浏览器存在、到调用才失败。
    pub fn available_on(&self, host: Host) -> bool {
        host.supports(self.transport_kind())
    }
}

/// `.mcp.json` 解析失败。撞名单独一个变体，方便「撞名 → 明确报错」被断言判定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    /// 不是合法 JSON、`mcpServers` 结构不对、或某个 stdio 条目缺 `command`。
    Parse(String),
    /// 同一个 server id 在 `mcpServers` 里声明了两次（不静默取后者）。
    DuplicateServerId(String),
    /// 读配置文件失败（[`McpConfig::from_file`] 路径）。
    Io(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Parse(m) => write!(f, ".mcp.json 解析失败: {m}"),
            ConfigError::DuplicateServerId(id) => {
                write!(f, ".mcp.json 里 server id `{id}` 声明了两次（撞名，不静默取后者）")
            }
            ConfigError::Io(m) => write!(f, "读 .mcp.json 失败: {m}"),
        }
    }
}

impl std::error::Error for ConfigError {}

impl McpConfig {
    /// 从本地路径读并解析。文件不存在等 IO 错 → [`ConfigError::Io`]（调用方——比如
    /// CLIbootstrap——自行决定「没有 .mcp.json」是不是要紧；M6 的判断是不要紧，正常起、
    /// 就是没有 MCP 工具，见 045）。
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io(e.to_string()))?;
        parse_config(&text)
    }
}

/// 解析一段 `.mcp.json` 文本。保序、保留重复 key（撞名靠这个才查得出），远端形状不
/// 让整份失败。
pub fn parse_config(json: &str) -> Result<McpConfig, ConfigError> {
    let root: RawRoot = serde_json::from_str(json).map_err(|e| ConfigError::Parse(e.to_string()))?;

    let mut seen = BTreeSet::new();
    let mut servers = Vec::with_capacity(root.mcp_servers.0.len());
    for (id, raw) in root.mcp_servers.0 {
        if !seen.insert(id.clone()) {
            return Err(ConfigError::DuplicateServerId(id));
        }
        let cfg = classify(&id, raw)?;
        servers.push((id, cfg));
    }
    Ok(McpConfig { servers })
}

// ── wire 形状（`.mcp.json`）：只在解析期存在，不外露 ──────────────────────────

#[derive(Deserialize)]
struct RawRoot {
    #[serde(rename = "mcpServers", default)]
    mcp_servers: RawServers,
}

/// `mcpServers` 对象流式解析成**保序、保留重复**的键值对——不经 `serde_json::Value`
/// 的 `Map`（那个撞 key 静默取后者，撞名就查不出了）。
#[derive(Default)]
struct RawServers(Vec<(String, RawServer)>);

impl<'de> Deserialize<'de> for RawServers {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Vec<(String, RawServer)>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("一个 mcpServers 对象")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = map.next_entry::<String, RawServer>()? {
                    out.push((k, v)); // 逐条收，不去重——撞名交给 parse_config 显式报。
                }
                Ok(out)
            }
        }
        Ok(RawServers(d.deserialize_map(V).map_err(de::Error::custom)?))
    }
}

/// 单个 server 条目的 wire 字段。未知字段默认忽略（协议向前兼容，server 可能带我们
/// 还不认识的字段）。
#[derive(Deserialize)]
struct RawServer {
    #[serde(rename = "type")]
    transport_type: Option<String>,
    command: Option<String>,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
}

/// 把一个 wire 条目分类成 stdio 还是远端。`type` 缺省当 stdio（Claude Code 惯例）；
/// `http`/`sse`（及任何非 stdio 的 type）当远端留位——**远端不让整份失败**。stdio
/// 缺 `command` 是真的坏配置 → `Parse`。
fn classify(id: &str, raw: RawServer) -> Result<ServerConfig, ConfigError> {
    match raw.transport_type.as_deref() {
        Some("stdio") | None => {
            let command = raw.command.ok_or_else(|| {
                ConfigError::Parse(format!("server `{id}` 是 stdio 形状但缺 command 字段"))
            })?;
            Ok(ServerConfig::Stdio(StdioServer { command, args: raw.args, env: raw.env }))
        }
        Some(other) => Ok(ServerConfig::Remote(RemoteServer {
            transport_type: other.to_string(),
            url: raw.url.unwrap_or_default(),
            headers: raw.headers,
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stdio_entry_parses_command_args_env() {
        let cfg = parse_config(
            r#"{"mcpServers":{"a":{"command":"npx","args":["-y","pkg"],"env":{"K":"V"}}}}"#,
        )
        .unwrap();
        assert_eq!(cfg.servers.len(), 1);
        let (id, ServerConfig::Stdio(s)) = &cfg.servers[0] else { panic!("应是 stdio") };
        assert_eq!(id, "a");
        assert_eq!(s.command, "npx");
        assert_eq!(s.args, vec!["-y", "pkg"]);
        assert_eq!(s.env_pairs(), vec![("K".to_string(), "V".to_string())]);
    }

    #[test]
    fn missing_optional_fields_default_empty() {
        let cfg = parse_config(r#"{"mcpServers":{"a":{"command":"c"}}}"#).unwrap();
        let (_, ServerConfig::Stdio(s)) = &cfg.servers[0] else { panic!() };
        assert!(s.args.is_empty());
        assert!(s.env.is_empty());
    }

    #[test]
    fn stdio_missing_command_is_a_parse_error() {
        let err = parse_config(r#"{"mcpServers":{"a":{"args":["x"]}}}"#).unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)), "实际 {err:?}");
    }

    #[test]
    fn empty_or_absent_mcp_servers_is_ok_and_empty() {
        assert!(parse_config("{}").unwrap().servers.is_empty());
        assert!(parse_config(r#"{"mcpServers":{}}"#).unwrap().servers.is_empty());
    }

    #[test]
    fn not_json_is_a_parse_error() {
        assert!(matches!(parse_config("not json").unwrap_err(), ConfigError::Parse(_)));
    }

    #[test]
    fn config_is_serializable_roundtrip() {
        let cfg = parse_config(
            r#"{"mcpServers":{"a":{"command":"c","args":["x"],"env":{"K":"V"}},
               "b":{"type":"http","url":"https://x","headers":{"H":"1"}}}}"#,
        )
        .unwrap();
        let s = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<McpConfig>(&s).unwrap(), cfg);
    }
}
