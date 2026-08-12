//! 装配 + 起服务 + 信号优雅退出（issue 035）：provider 装配交给
//! `agent_server::bootstrap`（跟 `examples/serve.rs` 共用同一份逻辑，见那个
//! 函数的模块文档），这个文件只管这个 bin 独有的部分——`--sessions-dir`/
//! `--port`/`--config`/`--ready-file` 四个 flag 怎么落地成 `bootstrap`/`bind`
//! 的输入、启动横幅打什么、Ctrl-C 或 Unix SIGTERM 触发时怎么收尾。`main.rs` 只解析参数、调
//! [`run`]，装配细节不出现在那个文件里。

use std::io::BufRead;
use std::sync::Arc;

use agent_core::SystemChunk;
use agent_server::{
    AgentServer, BootstrapError, BootstrapOptions, ServerConfig, ToolTableSpec, bootstrap,
    default_bind_addr,
};
use agent_transport::Client;
use tracing::{error, info};

use crate::{agent_limits, cli::Cli, ready_file, remote_tool_timeout};

/// 跟 `agent-cli`/`examples/serve.rs` 一字不差——三个宿主对「一个简洁诚实的
/// 助手」这句话的判断此前各写一遍，这不是巧合，是这个仓库目前唯一的默认
/// 人设，还没有出现需要三个宿主各说各话的场景。
const SYSTEM_PROMPT: &str = "你是一个简洁、诚实的助手。";

pub async fn run(cli: Cli) {
    let private_capability = read_private_capability(&cli);
    if let Some(path) = &cli.config {
        // SAFETY: 这一刻 main 函数刚起步、`#[tokio::main]` 的执行器还没起
        // 第二个线程——单线程期间改进程级环境变量没有并发风险
        // （`agent_server::bootstrap` 模块文档「`--config <path>` 的落地
        // 方式」：`agent_transport::config::load` 认 `AGENT_PROVIDERS_CONFIG`，
        // 且优先于 `./providers.toml`，这里借用同一条路径,不用再造参数通道）。
        unsafe { std::env::set_var("AGENT_PROVIDERS_CONFIG", path) };
    }

    // 内置工具的路径监狱根——跟 `agent-cli`/`examples/serve.rs` 一样锁在启动
    // 时的当前工作目录，多一层子目录（`SessionTemplate::open_spec` 现造
    // `tools_root/<session-id>/`）避免不同 session 互相踩脚。
    let tools_root = std::env::current_dir()
        .unwrap_or_else(|_| fail("working_directory", "agent-server startup failed"))
        .join(".agent-server-tools");

    // 决策 32：上限配错了**拒绝启动**，不静默退默认档（`agent_limits::parse_count`
    // 的文档记了这一取向跟的是 `AGENT_BIND` 而不是 `--port`）。跟
    // `remote_tool_timeout` 同一个出口形状：`Result` 交给 `fail` 变非零退出。
    let spawn_limits = agent_limits::from_cli_and_environment(&cli).unwrap_or_else(|message| {
        eprintln!("{message}");
        fail("agent_limits_config", "agent-server startup failed")
    });

    let assembled = bootstrap(BootstrapOptions {
        tools_root,
        default_sessions_dir: cli.sessions_dir.clone(),
        // 开满档：内置只读集 + `srv:shell/exec` + `srv:agent/spawn`，跟
        // `examples/serve.rs`/`agent-cli` 同一个「开箱即跑」判断——决策 12
        // 说的「用它就是开箱即跑」，不该是一个功能缩水的默认档位。
        //
        // 161：上限从 `--max-agent-depth`/`--max-children`（env 兜底）来，不再写死
        // 默认档。喂进这一档之后全链自动通：`ToolTableSpec::spawn_limits()` 读口把
        // 「工具描述里给模型看的那份」和「`spawn_child` 真正拦人的那份」对齐（034），
        // 恢复路径由 `actor::body` 转给 `recover` 的 `limits` 入参（160）。
        tools: ToolTableSpec::Full { spawn_limits },
        system: vec![SystemChunk {
            label: Arc::from("base"),
            text: Arc::from(SYSTEM_PROMPT),
        }],
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: None,
        provider_timeout: None,
        remote_tool_timeout: remote_tool_timeout::from_environment()
            .unwrap_or_else(|_| fail("remote_tool_timeout_config", "agent-server startup failed")),
        // s5：上传端点 + `srv:vision/inspect` 的临时目录——进程临时目录，进程
        // 退出即丢由 OS 回收（M3 单副本，不需要清理任务）。带 pid 防止多实例
        // 共用同一个目录互相覆盖。
        upload_dir: Some(
            std::env::temp_dir().join(format!("agent-uploads-{}", std::process::id())),
        ),
    })
    .unwrap_or_else(|error| {
        fail(
            bootstrap_failure_class(&error),
            "agent-server startup failed",
        )
    });

    let port = cli
        .port
        .or_else(|| {
            std::env::var("AGENT_SERVER_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(0);
    // 红线 8：地址走 `agent_server::default_bind_addr`（默认 loopback，
    // `AGENT_BIND` 显式覆盖），这个文件里不出现全零地址字面量。
    let addr = default_bind_addr(port)
        .unwrap_or_else(|_| fail("bind_config", "agent-server startup failed"));

    let provider_name = assembled.provider_name.clone();
    let model = assembled.template.model.clone();
    let config =
        ServerConfig::new(assembled.template).with_execution_bindings(assembled.execution_bindings);
    let config = match private_capability {
        Some(capability) => config.with_private_capability(capability),
        None => config,
    };
    let server = AgentServer::new(config);
    // 优雅关闭用的把手——在 `bind` 消费掉 `server` 之前先借出来
    // （`AgentServer::sessions` 文档）。
    let sessions = server.sessions();
    let bound = server
        .bind(addr)
        .await
        .unwrap_or_else(|_| fail("bind", "agent-server startup failed"));

    if let Some(path) = &cli.ready_file {
        ready_file::publish(path, bound.local_addr().port())
            .unwrap_or_else(|_| fail("ready_file", "agent-server startup failed"));
    }

    info!(
        listen_addr = %bound.local_addr(),
        provider = provider_name.as_str(),
        model = model.as_ref(),
        session_count = sessions.ids().len(),
        "agent-server 监听 http://{}",
        bound.local_addr()
    );

    tokio::select! {
        result = bound.serve() => {
            if result.is_err() {
                fail("serve", "agent-server stopped with a server failure");
            }
        }
        signal = shutdown_signal() => {
            info!(
                signal = signal.description(),
                session_count = sessions.ids().len(),
                "{}：优雅关闭全部会话（落盘快照）...",
                signal.description()
            );
            // `close_all` 内部 `join` 每个 actor 线程，是阻塞调用——扔进
            // `spawn_blocking`，不占用 tokio 的异步 worker（`SessionsHandle::
            // close_all` 文档同一条建议）。
            let outcomes = match tokio::task::spawn_blocking(move || sessions.close_all()).await {
                Ok(outcomes) => outcomes,
                Err(_) => {
                    error!(failure_class = "session_close_task", "agent server session shutdown failed");
                    Vec::new()
                }
            };
            let mut close_failures = 0usize;
            for (_, outcome) in &outcomes {
                if outcome.is_err() {
                    close_failures += 1;
                    error!(failure_class = "session_close", "agent server session shutdown failed");
                }
            }
            info!(
                closed_session_count = outcomes.len(),
                close_failure_count = close_failures,
                "agent server graceful shutdown complete"
            );
        }
    }
}

fn read_private_capability(cli: &Cli) -> Option<String> {
    if !cli.private_capability_stdin {
        return None;
    }
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .unwrap_or_else(|_| {
            fail(
                "private_capability_input",
                "private API capability is invalid",
            )
        });
    let capability = line.trim_end_matches(['\r', '\n']);
    if read == 0
        || capability.len() != 43
        || !capability
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        fail(
            "private_capability_input",
            "private API capability is invalid",
        );
    }
    Some(capability.to_owned())
}

enum ShutdownSignal {
    CtrlC,
    Sigterm,
}

impl ShutdownSignal {
    fn description(&self) -> &'static str {
        match self {
            Self::CtrlC => "Ctrl-C",
            Self::Sigterm => "SIGTERM",
        }
    }
}

async fn shutdown_signal() -> ShutdownSignal {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sigterm = signal(SignalKind::terminate())
            .unwrap_or_else(|_| fail("sigterm_listener", "agent-server shutdown failed"));
        tokio::select! {
            _ = tokio::signal::ctrl_c() => ShutdownSignal::CtrlC,
            _ = sigterm.recv() => ShutdownSignal::Sigterm,
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .unwrap_or_else(|_| fail("ctrl_c_listener", "agent-server shutdown failed"));
        ShutdownSignal::CtrlC
    }
}

fn bootstrap_failure_class(error: &BootstrapError) -> &'static str {
    match error {
        BootstrapError::Config(_) => "provider_config",
        BootstrapError::UnknownProvider(_) => "provider_selection",
        BootstrapError::MissingApiKey | BootstrapError::MissingExecutionProfileApiKey(_) => {
            "provider_authentication"
        }
        BootstrapError::UnknownExecutionProfileProvider { .. } => "execution_profile",
    }
}

fn fail(failure_class: &'static str, message: &str) -> ! {
    error!(failure_class, "agent server lifecycle failure");
    eprintln!("{message}");
    std::process::exit(1);
}
