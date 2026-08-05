//! 装配 + 起服务 + 信号优雅退出（issue 035）：provider 装配交给
//! `agent_server::bootstrap`（跟 `examples/serve.rs` 共用同一份逻辑，见那个
//! 函数的模块文档），这个文件只管这个 bin 独有的部分——`--sessions-dir`/
//! `--port`/`--config`/`--ready-file` 四个 flag 怎么落地成 `bootstrap`/`bind`
//! 的输入、启动横幅打什么、Ctrl-C 或 Unix SIGTERM 触发时怎么收尾。`main.rs` 只解析参数、调
//! [`run`]，装配细节不出现在那个文件里。

use std::sync::Arc;

use agent_core::{AgentLimits, SystemChunk};
use agent_server::{
    AgentServer, BootstrapOptions, BoundAgentServer, ServerConfig, ToolTableSpec, bootstrap,
    default_bind_addr,
};
use agent_transport::Client;

use crate::{cli::Cli, ready_file, remote_tool_timeout};

/// 跟 `agent-cli`/`examples/serve.rs` 一字不差——三个宿主对「一个简洁诚实的
/// 助手」这句话的判断此前各写一遍，这不是巧合，是这个仓库目前唯一的默认
/// 人设，还没有出现需要三个宿主各说各话的场景。
const SYSTEM_PROMPT: &str = "你是一个简洁、诚实的助手。";

pub async fn run(cli: Cli) {
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
        .unwrap_or_else(|e| fail(&format!("拿不到当前工作目录: {e}")))
        .join(".agent-server-tools");

    let assembled = bootstrap(BootstrapOptions {
        tools_root,
        default_sessions_dir: cli.sessions_dir.clone(),
        // 开满档：内置只读集 + `srv:shell/exec` + `srv:agent/spawn`，跟
        // `examples/serve.rs`/`agent-cli` 同一个「开箱即跑」判断——决策 12
        // 说的「用它就是开箱即跑」，不该是一个功能缩水的默认档位。
        tools: ToolTableSpec::Full {
            spawn_limits: AgentLimits::default(),
        },
        system: vec![SystemChunk {
            label: Arc::from("base"),
            text: Arc::from(SYSTEM_PROMPT),
        }],
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: None,
        provider_timeout: None,
        remote_tool_timeout: remote_tool_timeout::from_environment().unwrap_or_else(|e| fail(&e)),
    })
    .unwrap_or_else(|e| fail(&format!("{e}")));

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
    let addr = default_bind_addr(port).unwrap_or_else(|e| fail(&format!("{e}")));

    let provider_name = assembled.provider_name.clone();
    let model = assembled.template.model.clone();
    let server = AgentServer::new(ServerConfig::new(assembled.template));
    // 优雅关闭用的把手——在 `bind` 消费掉 `server` 之前先借出来
    // （`AgentServer::sessions` 文档）。
    let sessions = server.sessions();
    let bound = server
        .bind(addr)
        .await
        .unwrap_or_else(|e| fail(&format!("绑定 {addr} 失败: {e}")));

    if let Some(path) = &cli.ready_file {
        ready_file::publish(path, bound.local_addr().port())
            .unwrap_or_else(|e| fail(&format!("发布就绪文件 {} 失败: {e}", path.display())));
    }

    print_banner(&bound, &provider_name, &model, &cli);

    tokio::select! {
        result = bound.serve() => {
            if let Err(e) = result {
                fail(&format!("serve 失败: {e}"));
            }
        }
        signal = shutdown_signal() => {
            eprintln!("\n{}：优雅关闭全部会话（落盘快照）...", signal.description());
            // `close_all` 内部 `join` 每个 actor 线程，是阻塞调用——扔进
            // `spawn_blocking`，不占用 tokio 的异步 worker（`SessionsHandle::
            // close_all` 文档同一条建议）。
            let outcomes = tokio::task::spawn_blocking(move || sessions.close_all()).await.unwrap_or_default();
            for (id, outcome) in &outcomes {
                if let Err(e) = outcome {
                    eprintln!("  session {id}：{e}");
                }
            }
            eprintln!("已关闭 {} 个会话，退出。", outcomes.len());
        }
    }
}

fn print_banner(bound: &BoundAgentServer, provider_name: &str, model: &str, cli: &Cli) {
    eprintln!(
        "agent-server 监听 http://{}（provider={provider_name} model={model} tools=builtin+shell+spawn，开满档）",
        bound.local_addr()
    );
    match &cli.sessions_dir {
        Some(dir) => eprintln!(
            "会话目录={}（POST /sessions 不带 session_path 时自动落盘到这里）",
            dir.display()
        ),
        None => eprintln!(
            "会话目录=（未指定，POST /sessions 不带 session_path 就是内存会话——用 --sessions-dir <dir> 落盘）"
        ),
    }
    if let Some(path) = &cli.ready_file {
        eprintln!(
            "就绪文件={}（成功 bind 后原子发布实际端口，供父进程读取）。",
            path.display()
        );
    }
    eprintln!("Ctrl-C 或 Unix SIGTERM 退出（优雅：所有会话落盘快照之后才退出）。");
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
            .unwrap_or_else(|e| fail(&format!("无法监听 SIGTERM: {e}")));
        tokio::select! {
            _ = tokio::signal::ctrl_c() => ShutdownSignal::CtrlC,
            _ = sigterm.recv() => ShutdownSignal::Sigterm,
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .unwrap_or_else(|e| fail(&format!("无法监听 Ctrl-C: {e}")));
        ShutdownSignal::CtrlC
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
