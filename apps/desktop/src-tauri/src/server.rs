//! 起内嵌 `AgentServer`：issue 036「Rust 侧 setup：随机 loopback 端口起
//! AgentServer（with_static_dir 指向打包进资源的 web dist）」。这个文件只干
//! 「读配置、拼装、绑端口、后台跑起来」这一条装配线，不管窗口导航（`lib.rs`）、
//! 不管缺配置时的提示页（`first_run.rs`）——那两件事都是调用方看到
//! [`StartError`] 之后才决定做的事。

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use agent_core::{AgentLimits, SystemChunk};
use agent_server::{AgentServer, BootstrapError, BootstrapOptions, ServerConfig, SessionsHandle, ToolTableSpec, bootstrap};
use agent_transport::Client;
use tauri::AppHandle;

use crate::{dist, paths};

/// 起完之后调用方要的两样东西：主窗口该导航去哪、退出时怎么优雅关会话。
pub struct Started {
    pub addr: SocketAddr,
    pub sessions: SessionsHandle,
}

#[derive(Debug)]
pub enum StartError {
    /// 拿不到平台标准目录（`dirs` 在这台机器上解不出来——极罕见,`Error::
    /// UnknownPath` 见 tauri path 模块）。
    Path(tauri::Error),
    /// 找不到 `packages/web` 的构建产物。
    Dist(dist::DistNotFound),
    /// `providers.toml` 没有/读不了/没配 key——`lib.rs` 拿这个分支去挑
    /// 「首启提示页」的文案（[`BootstrapError`] 的 `Display` 已经是给人看的
    /// 完整诊断）。
    Bootstrap(BootstrapError),
    /// 造目录、绑端口这类 IO 失败。
    Io(std::io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::Path(e) => write!(f, "拿不到平台标准目录: {e}"),
            StartError::Dist(e) => write!(f, "{e}"),
            StartError::Bootstrap(e) => write!(f, "{e}"),
            StartError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// 缺配置（文件没有/没 key）跟其余失败要分开处理——`lib.rs` 只对这一类展示
/// 「把 providers.toml 放哪」的首启提示页,其余算宿主环境问题,日志报错、
/// 不假装成配置问题。
impl StartError {
    pub fn is_missing_config(&self) -> bool {
        matches!(self, StartError::Bootstrap(_))
    }
}

/// 读 `providers.toml`（平台标准目录，见 [`paths`]）→
/// `agent_server::bootstrap` 拼 `SessionTemplate` → 挂上
/// `with_static_dir`（[`dist::resolve`]）→ 绑**写死的** loopback、系统选口
/// （红线 8 桌面形态：不读 `AGENT_BIND`,桌面场景没有部署方需要覆盖的理由，
/// 这里干脆不给这条路）→ 后台跑起来。
pub async fn start(app: &AppHandle) -> Result<Started, StartError> {
    let providers_toml = paths::providers_toml_path(app).map_err(StartError::Path)?;
    // SAFETY：`setup()` 钩子在事件循环真正开始之前、单线程地跑一次，这是进程
    // 生命周期里唯一一次写这个变量；后面只有 `agent_transport::config::load`
    // （在这次 `bootstrap` 调用内部）读它，读写之间没有并发窗口。
    unsafe { std::env::set_var("AGENT_PROVIDERS_CONFIG", &providers_toml) };

    let dist_dir = dist::resolve(app).map_err(StartError::Dist)?;

    let bootstrapped = bootstrap(BootstrapOptions {
        tools_root: paths::tools_root(app).map_err(StartError::Path)?,
        default_sessions_dir: Some(paths::sessions_dir(app).map_err(StartError::Path)?),
        // 跟 `examples/serve.rs` 同一档——开满内置只读集 + shell + spawn，
        // 「与 web 版行为一致」（issue 036 验收）指的正是同一套工具表。
        tools: ToolTableSpec::Full { spawn_limits: AgentLimits::default() },
        system: vec![SystemChunk { label: Arc::from("base"), text: Arc::from("你是一个简洁、诚实的助手。") }],
        client: Arc::new(Client::new()),
        history_cap: None,
        snapshot_every: None,
        provider_timeout: None,
    })
    .map_err(StartError::Bootstrap)?;

    let config = ServerConfig::new(bootstrapped.template).with_static_dir(dist_dir);
    let server = AgentServer::new(config);

    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    let bound = server.bind(addr).await.map_err(StartError::Io)?;
    let local_addr = bound.local_addr();
    let sessions = bound.sessions();

    tauri::async_runtime::spawn(async move {
        if let Err(e) = bound.serve().await {
            log::error!("内嵌 agent-server 停了: {e}");
        }
    });

    Ok(Started { addr: local_addr, sessions })
}
