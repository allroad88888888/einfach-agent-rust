//! 命令行参数（issue 035）：手写解析，不上 clap——三个 flag（`--config`/
//! `--sessions-dir`/`--port`）加一个 `--help`，`agent_cli::session_path::
//! resolve` 那套「遍历 args，认 `--flag value` 和 `--flag=value` 两种写法，
//! 找不到就退环境变量」的手法已经证明够用，clap 换来的子命令/自动补全这里
//! 全用不上——issue 035「注意」条目原话「clap 或手写 args 自选（依赖最小
//! 优先）」，选的是不加依赖这条路。
//!
//! **不用 `#[derive(Parser)]` 也不用第三方 arg crate**：`agent-cli` 的
//! `session_path.rs` 是本仓已有的先例，这里的形状照抄它，只是要认的 flag
//! 从一个变成三个，多包一层 [`Cli`] struct 把它们收在一起。

use std::path::PathBuf;

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
}

pub enum ParsedArgs {
    /// `--help`/`-h` 出现在任意位置——不装配、不读 providers.toml，直接印
    /// 用法退出（`--help` 可读是 035 的验收条目之一，不该因为当前目录没有
    /// `providers.toml` 就先报一个无关的配置错误）。
    Help,
    Run(Cli),
}

/// `args` 收 `std::env::args().collect::<Vec<_>>()`（含 `argv[0]`）——不自己
/// 读 `std::env::args()`，测试要能喂一份夹具参数，跟 `agent_cli::session_path::
/// resolve` 同一个理由。
pub fn parse(args: &[String]) -> ParsedArgs {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return ParsedArgs::Help;
    }
    let mut cli = Cli { config: None, sessions_dir: None, port: None };
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
    -h, --help                打印这条帮助然后退出

ENV:
    AGENT_PROVIDERS_CONFIG    同 --config，命令行参数优先
    AGENT_SERVER_PORT         同 --port，命令行参数优先
    AGENT_BIND                覆盖监听地址（默认只绑 127.0.0.1，红线 8：监听
                              全部网卡必须显式设这个变量）

Ctrl-C 优雅退出：收到信号后关闭全部会话（落盘快照），再退出进程。
";

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn run_or_panic(parsed: ParsedArgs) -> Cli {
        match parsed {
            ParsedArgs::Run(cli) => cli,
            ParsedArgs::Help => panic!("期望 Run，拿到 Help"),
        }
    }

    #[test]
    fn no_flags_is_all_none() {
        let cli = run_or_panic(parse(&args(&["agent-server"])));
        assert!(cli.config.is_none());
        assert!(cli.sessions_dir.is_none());
        assert!(cli.port.is_none());
    }

    #[test]
    fn two_token_form_is_recognized_for_all_three_flags() {
        let cli = run_or_panic(parse(&args(&[
            "agent-server",
            "--config",
            "/tmp/providers.toml",
            "--sessions-dir",
            "/tmp/sessions",
            "--port",
            "8080",
        ])));
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/providers.toml")));
        assert_eq!(cli.sessions_dir, Some(PathBuf::from("/tmp/sessions")));
        assert_eq!(cli.port, Some(8080));
    }

    #[test]
    fn equals_form_is_recognized() {
        let cli = run_or_panic(parse(&args(&["agent-server", "--config=/x.toml", "--sessions-dir=/y", "--port=9"])));
        assert_eq!(cli.config, Some(PathBuf::from("/x.toml")));
        assert_eq!(cli.sessions_dir, Some(PathBuf::from("/y")));
        assert_eq!(cli.port, Some(9));
    }

    #[test]
    fn help_flag_short_circuits_before_anything_else() {
        assert!(matches!(parse(&args(&["agent-server", "--help"])), ParsedArgs::Help));
        assert!(matches!(parse(&args(&["agent-server", "-h"])), ParsedArgs::Help));
        // --help 出现在别的 flag 之后也一样识别——不要求它必须在最前面。
        assert!(matches!(parse(&args(&["agent-server", "--port", "1", "--help"])), ParsedArgs::Help));
    }

    #[test]
    fn unparseable_port_is_silently_none_not_a_panic() {
        // 跟 `agent_cli::session_path` 同一个取向：命令行解析层不做验证性
        // 报错，交给下游（这里是 `default_bind_addr`）在真正需要这个值时
        // 报出「配置错了」——避免解析器和使用者各有一套错误文案。
        let cli = run_or_panic(parse(&args(&["agent-server", "--port", "not-a-number"])));
        assert_eq!(cli.port, None);
    }
}
