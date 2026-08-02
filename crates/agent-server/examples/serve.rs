//! 起一个 agent-server 进程：读 `providers.toml`、装配 `SessionTemplate`、
//! 绑 loopback、打印监听地址。**不是 bin**——`examples/` 下的示例，035 才有
//! 真正的宿主二进制 `agent-server-bin`（issue 033「注意」：example 不违背
//! 「bin 是 M4」；035 落地之后这份示例继续留着，联调用途跟 bin 不同——033 的
//! `packages/web` 拿它当本地联调用的上游：`cargo run -p agent-server
//! --example serve`，不想装 `agent-server-bin` 也能跑）。
//!
//! 装配逻辑（读配置、选 provider、查 key、拼 `SessionTemplate`）换成
//! [`agent_server::bootstrap`]——035 issue「注意」条目原话：「bin 里重复的
//! 装配逻辑若超过三十行，提库函数，example 一起换用」，这份示例是那句话点名
//! 要换用的另一半（`agent-server-bin` 的 `src/main.rs` 是前一半）。两个宿主
//! 现在读同一份配置加载/错误文案，不会因为各自抄一遍而慢慢分叉。
//!
//! 端口：`AGENT_SERVER_PORT` 指定就用那个；不指定 = `0`（操作系统挑一个
//! 空闲端口，`bind()` 之后 `local_addr()` 才知道真实端口——见
//! `crate::http::AgentServer::bind` 文档「为什么拆成两步」）。红线 8：地址走
//! `agent_server::default_bind_addr`，这个文件里不出现全零地址字面量；要
//! 监听所有网卡（企业部署，不是本地联调）显式设 `AGENT_BIND`。
//!
//! 工具表开满档：`ToolTableSpec::Full { spawn_limits }`（034 补的第三档
//! ——`agent-server` 库以前只有 `Builtin`/`WithShell` 两档，没有对应 029 的
//! `agent_runtime::ToolTable::with_spawn(limits)`，`srv:agent/spawn` 经 HTTP
//! 链路连不上，033 实做记录的异议之一）。`spawn_limits` 用
//! `AgentLimits::default()`——跟 029 的默认（深度 ≤3、子数 ≤8）同一组数，
//! `ToolTableSpec::spawn_limits` 文档记了「不能只改这一侧数字」的耦合。
//!
//! `--sessions-dir` 的自动落盘（035：`SessionTemplate::default_sessions_dir`）
//! 这份示例不设——联调场景一贯是内存会话，改这个行为不是这次改动的目的，
//! 要落盘会话文件联调用 `agent-server-bin --sessions-dir`。

use std::sync::Arc;

use agent_core::{AgentLimits, SystemChunk};
use agent_server::{AgentServer, BootstrapOptions, ServerConfig, ToolTableSpec, bootstrap, default_bind_addr};
use agent_transport::Client;

#[tokio::main]
async fn main() {
    // 内置工具的路径监狱根——每个 session 实际锁在 `tools_root/<session-id>/`
    // 之内（`SessionTemplate::open_spec` 现造），跟 agent-cli 一样锁在启动时
    // 的当前工作目录，只是多一层子目录避免不同 session 互相踩脚。
    let tools_root = std::env::current_dir().unwrap_or_else(|e| fail(&format!("拿不到当前工作目录: {e}"))).join(".agent-server-tools");

    let assembled = bootstrap(BootstrapOptions {
        tools_root,
        default_sessions_dir: None,
        tools: ToolTableSpec::Full { spawn_limits: AgentLimits::default() },
        system: vec![SystemChunk { label: Arc::from("base"), text: Arc::from("你是一个简洁、诚实的助手。") }],
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: None,
        provider_timeout: None,
    })
    .unwrap_or_else(|e| fail(&format!("{e}")));

    let port: u16 = std::env::var("AGENT_SERVER_PORT").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let addr = default_bind_addr(port).unwrap_or_else(|e| fail(&format!("{e}")));

    let provider_name = assembled.provider_name.clone();
    let model = assembled.template.model.clone();
    let bound = AgentServer::new(ServerConfig::new(assembled.template))
        .bind(addr)
        .await
        .unwrap_or_else(|e| fail(&format!("绑定 {addr} 失败: {e}")));

    eprintln!(
        "agent-server 监听 http://{}（provider={provider_name} model={model} tools=builtin+shell+spawn，开满档）",
        bound.local_addr(),
    );
    eprintln!("Ctrl-C 退出。把上面这个地址喂给 packages/web 的 AGENT_SERVER 环境变量（见包内 README）。");

    if let Err(e) = bound.serve().await {
        fail(&format!("serve 失败: {e}"));
    }
}

fn fail(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}
