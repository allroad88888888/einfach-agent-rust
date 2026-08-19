//! 命令行参数（issue 035）：手写解析，不上 clap——三个 flag（`--config`/
//! `--sessions-dir`/`--port`/`--ready-file`）加一个 `--help`，`agent_cli::session_path::
//! resolve` 那套「遍历 args，认 `--flag value` 和 `--flag=value` 两种写法，
//! 找不到就退环境变量」的手法已经证明够用，clap 换来的子命令/自动补全这里
//! 全用不上——issue 035「注意」条目原话「clap 或手写 args 自选（依赖最小
//! 优先）」，选的是不加依赖这条路。
//!
//! **不用 `#[derive(Parser)]` 也不用第三方 arg crate**：`agent-cli` 的
//! `session_path.rs` 是本仓已有的先例，这里的形状照抄它，只是要认的 flag
//! 从一个变成四个，多包一层 [`Cli`] struct 把它们收在一起。

use std::path::PathBuf;

use crate::agent_limits;

pub struct Cli {
    /// `--config <path>`：覆盖 providers.toml 的位置。`run::run` 把它写进
    /// `AGENT_PROVIDERS_CONFIG` 环境变量——`agent_transport::config::load`
    /// 本来就认这个变量、且优先于 `./providers.toml`（`agent_server::
    /// bootstrap` 模块文档「`--config <path>` 的落地方式」），这一层不用
    /// 再造一个参数通道直接传路径。
    pub config: Option<PathBuf>,
    /// `--sessions-dir <dir>`：`POST /sessions` 不带 `session_path` 时自动
    /// 落盘到 `<dir>/<id>.jsonl`（`SessionTemplate::default_sessions_dir`
    /// 语义，035 issue 原文）。不给就是旧行为：内存会话，进程退出即丢。
    pub sessions_dir: Option<PathBuf>,
    /// `--port <n>`；没给就退 `AGENT_SERVER_PORT` 环境变量（035 issue 原文
    /// 「端口 `--port`/`AGENT_SERVER_PORT`」）；两个都没有就是 `0`（操作系统
    /// 挑一个空闲端口）。
    pub port: Option<u16>,
    /// `--ready-file <path>`：成功 bind 后原子发布端口、pid 与版本，让父进程
    /// 不必把人类日志当协议解析。文件协议及原子写入细节在 `ready_file` 模块。
    pub ready_file: Option<PathBuf>,
    /// 从 stdin 单行读取 Java 为此子进程随机生成的 private API capability。
    pub private_capability_stdin: bool,
    /// `--max-agent-depth <n>`：子 agent 树的深度上限（root = 0，默认 3）。
    /// 没给就退 `AGENT_MAX_AGENT_DEPTH`（决策 32；解析与兜底都在
    /// [`crate::agent_limits`]，这里只记「命令行上给没给」）。
    pub max_agent_depth: Option<usize>,
    /// `--max-children <n>`：每个 agent 活着的直接子 agent 数上限（默认 8）。
    /// 没给就退 `AGENT_MAX_CHILDREN`。
    pub max_children: Option<usize>,
    /// `--max-auto-turns <n>`：一次用户输入之后，会话**自己**还能往下开几轮
    /// （211，决策 35 §二；默认 3，**0 = 关掉自驱动**）。没给就退
    /// `AGENT_MAX_AUTO_TURNS`。
    ///
    /// 跟上面两道闸量的不是同一件事：它们量「树有多大」，这一道量「没人看着时
    /// 还能自己跑几轮」——部署方估账时把三个数连同 `MaxTurns` 一起相乘。
    pub max_auto_turns: Option<u32>,
}

pub enum ParsedArgs {
    /// `--help`/`-h` 出现在任意位置——不装配、不读 providers.toml，直接印
    /// 用法退出（`--help` 可读是 035 的验收条目之一，不该因为当前目录没有
    /// `providers.toml` 就先报一个无关的配置错误）。
    Help,
    Invalid(String),
    Run(Cli),
}

/// `args` 收 `std::env::args().collect::<Vec<_>>()`（含 `argv[0]`）——不自己
/// 读 `std::env::args()`，测试要能喂一份夹具参数，跟 `agent_cli::session_path::
/// resolve` 同一个理由。
pub fn parse(args: &[String]) -> ParsedArgs {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return ParsedArgs::Help;
    }
    let mut cli = Cli {
        config: None,
        sessions_dir: None,
        port: None,
        ready_file: None,
        private_capability_stdin: false,
        max_agent_depth: None,
        max_children: None,
        max_auto_turns: None,
    };
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if let Some(v) = arg.strip_prefix("--config=") {
            cli.config = Some(PathBuf::from(v));
        } else if arg == "--config" {
            i += 1;
            cli.config = args.get(i).map(PathBuf::from);
        } else if let Some(v) = arg.strip_prefix("--sessions-dir=") {
            cli.sessions_dir = Some(PathBuf::from(v));
        } else if arg == "--sessions-dir" {
            i += 1;
            cli.sessions_dir = args.get(i).map(PathBuf::from);
        } else if let Some(v) = arg.strip_prefix("--port=") {
            cli.port = v.parse().ok();
        } else if arg == "--port" {
            i += 1;
            cli.port = args.get(i).and_then(|v| v.parse().ok());
        } else if let Some(v) = arg.strip_prefix("--ready-file=") {
            cli.ready_file = Some(PathBuf::from(v));
        } else if arg == "--ready-file" {
            i += 1;
            cli.ready_file = args.get(i).map(PathBuf::from);
        } else if arg == "--private-capability-stdin" {
            cli.private_capability_stdin = true;
        } else if let Some(v) = arg.strip_prefix("--max-agent-depth=") {
            // 上限**解析不出来就拒绝启动**，不跟 `--port` 那样静默退 `None`
            // ——理由（有没有下游替它报错）在 `agent_limits::parse_count` 的文档。
            match agent_limits::parse_count("--max-agent-depth", Some(v)) {
                Ok(n) => cli.max_agent_depth = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if arg == "--max-agent-depth" {
            i += 1;
            match agent_limits::parse_count("--max-agent-depth", args.get(i).map(String::as_str)) {
                Ok(n) => cli.max_agent_depth = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if let Some(v) = arg.strip_prefix("--max-children=") {
            match agent_limits::parse_count("--max-children", Some(v)) {
                Ok(n) => cli.max_children = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if arg == "--max-children" {
            i += 1;
            match agent_limits::parse_count("--max-children", args.get(i).map(String::as_str)) {
                Ok(n) => cli.max_children = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if let Some(v) = arg.strip_prefix("--max-auto-turns=") {
            // **允许 0**（关掉自驱动），所以走 `parse_auto_turns` 而不是
            // `parse_count`——后者钉死下限 1，那是给两道结构闸的（见
            // `agent_limits` 模块文档）。
            match agent_limits::parse_auto_turns("--max-auto-turns", Some(v)) {
                Ok(n) => cli.max_auto_turns = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if arg == "--max-auto-turns" {
            i += 1;
            match agent_limits::parse_auto_turns("--max-auto-turns", args.get(i).map(String::as_str))
            {
                Ok(n) => cli.max_auto_turns = Some(n),
                Err(e) => return ParsedArgs::Invalid(e),
            }
        } else if arg.starts_with('-') {
            return ParsedArgs::Invalid(format!("unknown option: {arg}"));
        }
        i += 1;
    }
    ParsedArgs::Run(cli)
}

pub const HELP: &str = "\
agent-server [OPTIONS]

Agent 运行时的默认 HTTP 宿主（企业内嵌 agent-server 这个库时不需要这个二进制，
见 docs/ARCHITECTURE.md 决策 12）。

OPTIONS:
    --config <path>          providers.toml 的位置（默认查找顺序：
                              $AGENT_PROVIDERS_CONFIG -> ./providers.toml ->
                              ~/.config/agent/providers.toml）
    --sessions-dir <dir>     POST /sessions 不带 session_path 时自动把会话落盘到
                              <dir>/<id>.jsonl；不给就是内存会话，进程退出即丢
    --port <n>                监听端口；不给就退 AGENT_SERVER_PORT 环境变量；
                              两个都没有就是 0（操作系统挑一个空闲端口）
    --ready-file <path>       成功 bind 后原子发布 JSON 就绪文件；内容含
                              port、pid、version，供父进程读取实际端口
    --private-capability-stdin 从 stdin 读取一行私有 API capability；Java 托管时必需
    --max-agent-depth <n>     子 agent 树的深度上限，root 是 0（默认 3）
    --max-children <n>        每个 agent 活着的直接子 agent 数上限（默认 8）
                              两者都要 ≥ 1；给了非法值直接拒绝启动，不退回默认值。
                              要整个关掉子 agent 请用建会话时的
                              capabilities.disable_builtin: [\"srv:agent/spawn\"]
    --max-auto-turns <n>      一次用户输入之后，会话**自己**还能往下开几轮（默认 3）。
                              留言（srv:agent/send 的 when=\"next_turn\"）靠它续跑；
                              **0 = 关掉自驱动**（这一项允许 0，上面两项不允许）。
                              这是三道量不同东西的闸里的第三道：上面两道量「树有
                              多大」，MaxTurns（每个会话自己的槽位）量「一轮里说
                              几次话」，这一道量「没人看着时能跑几轮」——估账把
                              三个数相乘
    -h, --help                打印这条帮助然后退出

ENV:
    AGENT_PROVIDERS_CONFIG    同 --config，命令行参数优先
    AGENT_SERVER_PORT         同 --port，命令行参数优先
    AGENT_MAX_AGENT_DEPTH     同 --max-agent-depth，命令行参数优先
    AGENT_MAX_CHILDREN        同 --max-children，命令行参数优先
    AGENT_MAX_AUTO_TURNS      同 --max-auto-turns，命令行参数优先
    AGENT_REMOTE_TOOL_TIMEOUT_MS
                              远程工具领取后等待结果的正整数毫秒数；不给则使用
                              运行时默认值 600000（10 分钟）
    AGENT_BIND                覆盖监听地址（默认只绑 127.0.0.1，红线 8：监听
                              全部网卡必须显式设这个变量）

Ctrl-C 或 Unix SIGTERM 优雅退出：收到信号后关闭全部会话（落盘快照），再退出进程。
";

/// 解析单测拆去 `cli_tests.rs`——161 加两个上限 flag 之后这个文件顶破了
/// 红线 9 的 300 行。同 `agent-core` 里 `spawn.rs`/`despawn.rs` 的先例。
#[cfg(test)]
#[path = "cli_tests.rs"]
mod tests;
