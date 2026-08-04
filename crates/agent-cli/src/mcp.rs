//! CLI 侧的 MCP 装载接线（045）。**这一件事**：把「启动目录下的 `.mcp.json`」
//! 变成 CLI 起 loop 需要的三样东西——追加进 `ToolTable` 的工具批（041 翻译产出的
//! `(ToolSpec, Reversibility)`）、进 `RunnerCtx` 的活句柄 [`McpRegistry`]（红线 3，
//! store 外），以及 `/mcp` 命令要展示的可序列化装载状态。
//!
//! 真正的多 server 装载 / 失败隔离 / host 门在 044 的 [`load_servers`] 里——这里只是
//! 宿主侧的**接线**：找配置文件、按 server id 归一顺序（红线 11）、把产物分成三路。
//!
//! # 缺失 / 坏配置不致命（045 验收）
//!
//! 没有 `.mcp.json` 是「就是没接 MCP」的正常情况，不报错、不崩、零 MCP 工具起。
//! `--mcp-config` 显式指了个不存在的文件、或配置解析失败（撞名 / 语法坏）→ 打一句
//! 警告后按无 MCP 继续（跟 `main.rs` 里 skill 装载失败退回空 registry 同一个精神）。
//!
//! # 崩溃恢复（045 §恢复重连）
//!
//! 这个 bootstrap **每次启动都跑**——句柄住 store 外，从不进快照，所以 kill-9 重启后
//! 会话历史从持久化恢复、MCP server 却是从 `.mcp.json` **重新 spawn** 的新子进程。
//! 恢复路径天然如此，无需任何额外代码（docs/MCP.md §「活句柄住 store 外」）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};
use agent_mcp::{Host, LoadTimeouts, McpConfig, McpRegistry, ServerStatus, load_servers};

/// 默认配置文件名，相对 CLI 启动目录解析。
pub const DEFAULT_MCP_CONFIG: &str = ".mcp.json";

/// 握手时本仓 client 的自报身份（loader 传给 `initialize`）。
const CLIENT_NAME: &str = "agent-cli";
const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 一次 MCP bootstrap 的产物，分三路给 CLI 装配：
/// - `tools` → `ToolTable::with_mcp`（追加在既有工具后，红线 11）
/// - `registry` → `RunnerCtx::with_mcp`（活句柄，红线 3）
/// - `status` → `/mcp` 命令展示（含起不来的 server 的原因，registry 里没有这份）
pub struct McpBootstrap {
    pub registry: Arc<McpRegistry>,
    pub tools: Vec<(ToolSpec, Reversibility)>,
    pub status: McpStatus,
}

/// `/mcp` 命令的数据源：每个 server 的装载期可用性 + 它暴露的工具名。
///
/// 用**装载期状态**而不是活 registry：起不来的 server（unavailable/unsupported）带着
/// 原因活在这里，从不进 registry；连上的 server 的工具名也在这里按 id 排 + tools/list
/// 顺序排好，`/mcp` 直接渲染不必反查 registry。
pub struct McpStatus {
    pub servers: Vec<ServerStatus>,
    pub tool_names: Vec<Arc<str>>,
}

impl McpBootstrap {
    fn empty(registry: Arc<McpRegistry>) -> Self {
        Self { registry, tools: Vec::new(), status: McpStatus { servers: Vec::new(), tool_names: Vec::new() } }
    }

    /// 启动横幅里那一行摘要。没配 server → 「（无）」。
    pub fn summary(&self) -> String {
        if self.status.servers.is_empty() {
            return "（无）".to_string();
        }
        let connected = self.status.servers.iter().filter(|s| s.is_connected()).count();
        format!("{}/{} server 连上，{} 个工具", connected, self.status.servers.len(), self.tools.len())
    }
}

/// 从参数列表解析 `--mcp-config <path>` / `--mcp-config=<path>`；没给就用启动目录下
/// 的默认 `.mcp.json`。返回 `(路径, 是否显式指定)`——显式指了但文件不存在要警告，
/// 默认缺失是「就是没有 MCP」的正常情况，不警告。收参数而不是自读 `std::env::args()`
/// （跟 [`crate::session_path::resolve`] 同一个手法），测试才能喂夹具参数。
pub fn resolve_config_path(args: &[String]) -> (PathBuf, bool) {
    for (i, arg) in args.iter().enumerate() {
        // 两种写法归一成 `Option<PathBuf>`：`--mcp-config <path>` 取下一个 token，
        // `--mcp-config=<path>` 取等号右边。末尾裸 `--mcp-config`（没跟值）→ None，
        // 落到循环外的默认路径。
        let path = if arg == "--mcp-config" {
            args.get(i + 1).map(PathBuf::from)
        } else {
            arg.strip_prefix("--mcp-config=").map(PathBuf::from)
        };
        if let Some(path) = path {
            return (path, true);
        }
    }
    (PathBuf::from(DEFAULT_MCP_CONFIG), false)
}

/// 读 + 装载一份 `.mcp.json`。缺失 / 坏配置不致命——见模块文档。`explicit` 决定默认
/// 路径缺失时静默还是警告。`warn` 收警告文案（`main.rs` 转 `eprintln!`，测试可捕获）。
pub fn bootstrap(path: &Path, explicit: bool, warn: &mut dyn FnMut(&str)) -> McpBootstrap {
    let registry = Arc::new(McpRegistry::new());
    if !path.exists() {
        if explicit {
            warn(&format!("--mcp-config 指向的文件不存在：{}（按无 MCP 继续）", path.display()));
        }
        return McpBootstrap::empty(registry);
    }
    let config = match McpConfig::from_file(path) {
        Ok(c) => c,
        Err(e) => {
            warn(&format!("{e}（按无 MCP 继续）"));
            return McpBootstrap::empty(registry);
        }
    };
    load(config, registry, LoadTimeouts::default(), warn)
}

/// 归一顺序 + 跑 loader + 分三路。抽出来是为了让测试能传短超时。
///
/// `warn` 在这里还要**再用一次**：loader 自己也会产出告警（074 的同名工具去重——
/// 一个 server 的 `tools/list` 里两项重名，后来的整条丢弃）。那些痕迹汇在
/// `LoadOutcome::warnings` 里，**必须在这一跳转出去**，否则就成了「记录下来但没人读」
/// ——对部署方而言跟静默丢弃没有区别，而 074 的整条验收就是「痕迹要到得了部署方」。
fn load(
    mut config: McpConfig,
    registry: Arc<McpRegistry>,
    timeouts: LoadTimeouts,
    warn: &mut dyn FnMut(&str),
) -> McpBootstrap {
    // 红线 11：server 之间按 id 排（server 内工具顺序 = `tools/list`，由 loader 保）。
    // 撞名已在 `parse_config` 拦掉，id 唯一，排序是全序、逐字节确定，不受 `.mcp.json`
    // 里书写顺序影响——追加进工具表的稳定前缀因此不漂。
    config.servers.sort_by(|a, b| a.0.cmp(&b.0));
    let outcome = load_servers(&config, &registry, Host::Server, timeouts, CLIENT_NAME, CLIENT_VERSION);
    for message in &outcome.warnings {
        warn(message);
    }
    let tool_names = outcome.tools.iter().map(|(s, _)| Arc::clone(&s.name)).collect();
    McpBootstrap { registry, tools: outcome.tools, status: McpStatus { servers: outcome.servers, tool_names } }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agent_mcp::{Availability, parse_config};

    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn resolve_default_when_no_flag() {
        let (path, explicit) = resolve_config_path(&args(&["agent-cli"]));
        assert_eq!(path, PathBuf::from(DEFAULT_MCP_CONFIG));
        assert!(!explicit);
    }

    #[test]
    fn resolve_two_token_and_equals_forms_are_explicit() {
        let (p1, e1) = resolve_config_path(&args(&["agent-cli", "--mcp-config", "/tmp/a.json"]));
        assert_eq!(p1, PathBuf::from("/tmp/a.json"));
        assert!(e1);
        let (p2, e2) = resolve_config_path(&args(&["agent-cli", "--mcp-config=/tmp/b.json"]));
        assert_eq!(p2, PathBuf::from("/tmp/b.json"));
        assert!(e2);
    }

    #[test]
    fn missing_default_config_starts_clean_and_silent() {
        let mut warned: Vec<String> = Vec::new();
        let boot =
            bootstrap(Path::new("/no/such/dir/.mcp.json"), false, &mut |m| warned.push(m.to_string()));
        assert!(boot.tools.is_empty(), "无配置 → 零 MCP 工具");
        assert!(boot.status.servers.is_empty());
        assert!(boot.status.tool_names.is_empty());
        assert!(warned.is_empty(), "默认缺失不该警告（就是没接 MCP 的正常情况）");
        assert_eq!(boot.summary(), "（无）");
    }

    #[test]
    fn explicit_missing_config_warns_but_still_starts_clean() {
        let mut warned: Vec<String> = Vec::new();
        let boot =
            bootstrap(Path::new("/no/such/dir/.mcp.json"), true, &mut |m| warned.push(m.to_string()));
        assert!(boot.tools.is_empty());
        assert_eq!(warned.len(), 1, "显式指了不存在的文件要警告一次");
    }

    /// 两个 server 命令都不存在 → 都 `Unavailable`（失败隔离，不崩），status 里 server
    /// 顺序按 id 排（不是 `.mcp.json` 书写顺序）、零工具。非网络：spawn 立刻 ENOENT。
    #[test]
    fn servers_sorted_by_id_and_failures_isolated() {
        let config = parse_config(
            r#"{"mcpServers":{
                "zeta":{"command":"__no_such_cmd_zeta__"},
                "alpha":{"command":"__no_such_cmd_alpha__"}
            }}"#,
        )
        .unwrap();
        let boot = load(
            config,
            Arc::new(McpRegistry::new()),
            LoadTimeouts { handshake: Duration::from_millis(500), call: Duration::from_millis(500) },
            &mut |_| {},
        );
        let ids: Vec<&str> = boot.status.servers.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "zeta"], "server 之间按 id 排，不受书写顺序影响");
        assert!(
            boot.status.servers.iter().all(|s| matches!(s.availability, Availability::Unavailable { .. })),
            "命令不存在 → Unavailable",
        );
        assert!(boot.tools.is_empty(), "起不来的 server 不贡献工具");
        assert!(!boot.registry.contains("zeta"), "半连的 client 不进 registry");
    }

    /// 远端 server 形状只解析、不装载 → `Unsupported`，同样不崩、不贡献工具。
    #[test]
    fn remote_server_is_unsupported_not_fatal() {
        let config =
            parse_config(r#"{"mcpServers":{"r":{"type":"http","url":"https://x"}}}"#).unwrap();
        let boot = load(config, Arc::new(McpRegistry::new()), LoadTimeouts::default(), &mut |_| {});
        assert!(matches!(boot.status.servers[0].availability, Availability::Unsupported { .. }));
        assert!(boot.tools.is_empty());
    }

    /// 074 的告警**真的走到 `warn` 出口**——这条测的不是去重本身（那在 `agent-mcp`
    /// 有四条），是「痕迹到不到得了部署方」这**最后一跳**。074 落地时被硬边界挡在
    /// `agent-mcp` 里，`LoadOutcome::warnings` 写好了却没有读者——对部署方而言跟
    /// 静默丢弃没区别。
    ///
    /// 必须用一个**真的会产出重复工具名**的假 server（`sh` 脚本，同
    /// `agent-mcp/tests/list_tools_duplicate_074.rs` 的手法）：拿连不上的 server 凑数
    /// 是产不出 warning 的，那种断言在转发循环被删掉之后照样绿——**对突变免疫的
    /// 测试等于没写**。
    #[test]
    fn loader_duplicate_tool_warning_reaches_the_warn_sink() {
        // `tools/list` 回两条同名 `echo` → loader 丢弃后来那条并产一条告警。
        // 单行 JSON：`read_line` 按 `\n` 切帧，混进换行会把一帧切成两半。
        let tools = r#"{"name":"echo","description":"first","inputSchema":{"type":"object"}},{"name":"echo","description":"second","inputSchema":{"type":"object"}}"#;
        let script = format!(
            "read l1\n\
             printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{{}}}}}}'\n\
             read l2\n\
             read l3\n\
             printf '%s\\n' '{{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{{\"tools\":[{tools}]}}}}'\n"
        );
        let config_json =
            serde_json::json!({"mcpServers": {"dup": {"command": "sh", "args": ["-c", script]}}})
                .to_string();
        let config = parse_config(&config_json).unwrap();

        let mut seen: Vec<String> = Vec::new();
        let boot = load(
            config,
            Arc::new(McpRegistry::new()),
            LoadTimeouts::default(),
            &mut |m| seen.push(m.to_string()),
        );

        assert_eq!(seen.len(), 1, "loader 那条 074 告警必须逐条转出去，实收：{seen:?}");
        assert!(seen[0].contains("dup"), "告警要带 server id，否则部署方不知道去改哪个配置：{}", seen[0]);
        assert!(seen[0].contains("echo"), "告警要带重复的工具名：{}", seen[0]);
        assert_eq!(boot.tools.len(), 1, "重复的那条整条不进工具表（074 的去重本体）");
    }
}
