//! issue 031：axum 挂在 030 的 `SessionRegistry`/`SessionHandle` 之上的 HTTP/SSE
//! 面。库形态不变（决策 12）——这个模块加了公开入口
//! [`AgentServer::new(config).serve(addr)`]（ARCHITECTURE.md §传输原文）。035
//! 加的 `agent-server-bin` 是这套面的众多宿主之一，不改这里的形态，只多用
//! [`AgentServer::sessions`] 这一个新读口。
//!
//! # 模块地图
//!
//! | 模块 | 管什么 |
//! |---|---|
//! | [`config`] | `ServerConfig`/`SessionTemplate`：`AgentServer::new` 的输入 |
//! | [`capabilities`] | issue 061：`POST /sessions` 里宿主声明的 tool/skill——协议形状 + 名字校验（纯数据，零 IO） |
//! | [`state`] | `AppState`：路由共享的东西，`AGENT_BIND` 无关，纯请求处理状态 |
//! | [`hub`] | SSE 环形缓冲 + 断开取消的引用计数与宽限计时 |
//! | [`routes`] | 六个端点 + 会话创建/查询的处理函数 |
//! | [`error`] | 统一的 `{"error":{"code","message"}}` 错误形状 |
//! | [`json`] | `Json<T>` 的入参替身，把 rejection 也并进统一错误形状 |
//! | [`static_files`] | issue 036：`with_static_dir` 的 SPA 兜底 service，API 路由优先 |
//! | [`sessions_handle`] | issue 035：`SessionsHandle`——宿主优雅关闭时枚举 + 关掉全部会话 |
//!
//! 红线 8（`bind` 默认 loopback）不在这个模块——那是 [`crate::bind`] 的事，
//! `AgentServer::bind`/`serve` 只是把调用方给的 `SocketAddr` 拿去
//! `TcpListener::bind`，默认值从哪来是调用方的选择（生产代码该用
//! `agent_server::default_bind_addr`，`crate::bind` 模块文档有理由）。

mod capabilities;
mod config;
mod error;
mod hub;
mod json;
mod pending;
mod poll_protocol;
mod routes;
mod sessions_handle;
mod state;
mod static_files;
mod tool_protocol;

pub use config::{ServerConfig, SessionTemplate};
pub use sessions_handle::SessionsHandle;

use std::io;
use std::net::SocketAddr;

use axum::Router;
use tokio::net::TcpListener;

use state::AppState;

/// 061：`POST /sessions` 请求体里的 `capabilities`——上行协议的一半，跟下行的
/// `Frame`/`PollResponse` 一起导出给前端（065 直接用生成的类型，不手写镜像）。
#[cfg(feature = "ts")]
pub(crate) use capabilities::Capabilities;
/// 072：`GET /sessions/{id}/pending_tools` 的响应体——宿主执行一次远端工具之前
/// 求证用的那份投影。跟 `Frame`/`PollResponse` 一起导出给前端。
#[cfg(feature = "ts")]
pub(crate) use pending::PendingToolsResponse;
#[cfg(feature = "ts")]
pub(crate) use poll_protocol::PollResponse;
#[cfg(feature = "ts")]
pub(crate) use tool_protocol::{
    ToolClaimRequest, ToolClaimResponse, ToolResultResponse, ToolResultV2Request,
    ToolStatusResponse,
};

/// `AgentServer::new(config)` 之后拿到的东西：路由已经装好，还没绑端口。
pub struct AgentServer {
    router: Router,
    /// `routes::router` 的输入被 move 走之后，这里留一份克隆——`AppState` 本身
    /// 是 `Arc` 包底的薄句柄，克隆不复制 registry（`sessions_handle` 模块文档）。
    /// 唯一的用途是 [`Self::sessions`]：宿主想在 `bind`/`serve` 消费掉 `self`
    /// 之前先拿到关闭用的把手。
    state: AppState,
}

impl AgentServer {
    /// 静态托管（issue 036）是可选的：`config.static_dir` 设了就把它挂成
    /// `fallback_service`——只有请求命中不了下面六个端点 + 会话创建/查询时
    /// 才会落到静态文件（[`static_files`] 模块文档「SPA 兜底怎么跟 API 路由
    /// 共存」）。没设就是纯 API 服务器，M3 之前的行为一字不变。
    pub fn new(config: ServerConfig) -> Self {
        let static_dir = config.static_dir.clone();
        let state = AppState::new(config);
        let router = routes::router(state.clone());
        let router = match static_dir {
            Some(dir) => router.fallback_service(static_files::spa_fallback(&dir)),
            None => router,
        };
        AgentServer { router, state }
    }

    /// 优雅关闭用的把手（issue 035）：枚举当前登记的 session、逐个 `close`
    /// 落盘快照。`&self`——`bind`/`serve` 消费 `self` 之前调用，宿主典型用法
    /// 是 `let sessions = server.sessions(); let bound = server.bind(addr).await?;`，
    /// 拿到的把手在 `bound.serve()` 跑起来之后依然指向同一份 registry（见
    /// [`SessionsHandle`] 模块文档）。
    pub fn sessions(&self) -> SessionsHandle {
        SessionsHandle::new(self.state.clone())
    }

    /// 绑定地址，返回知道自己实际监听在哪的句柄。**拆成 `bind` + `serve` 两步
    /// 而不是只给一个吃到底的 `serve(addr)`**：`addr` 的端口给 `0` 时由操作
    /// 系统现选，调用方（尤其测试——「假浏览器」得知道连哪个端口）必须在真正
    /// 开始服务之前就问到实际端口，一个直接跑到底的 `.await` 做不到这件事。
    /// [`Self::serve`] 就是这两步的合体，对应 ARCHITECTURE.md 原文那一行。
    pub async fn bind(self, addr: SocketAddr) -> io::Result<BoundAgentServer> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        Ok(BoundAgentServer {
            listener,
            router: self.router,
            local_addr,
            state: self.state,
        })
    }

    /// `AgentServer::new(config).serve(addr).await`——ARCHITECTURE.md §传输
    /// 原文的形态。跑到调用方主动结束这个 future 为止（进程退出、或者外面用
    /// `tokio::select!`/`Shutdown` 信号打断），没有内建的自动停止条件。
    pub async fn serve(self, addr: SocketAddr) -> io::Result<()> {
        self.bind(addr).await?.serve().await
    }
}

/// 已经绑好端口、还没开始服务的 [`AgentServer`]。
pub struct BoundAgentServer {
    listener: TcpListener,
    router: Router,
    local_addr: SocketAddr,
    state: AppState,
}

impl BoundAgentServer {
    /// 操作系统实际分配的地址——`addr` 端口给 `0` 时唯一能知道真实端口的办法。
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// 同 [`AgentServer::sessions`]，绑完地址之后才想要这个把手的话用这个——
    /// 两者返回的都是指向同一份 registry 的句柄，先后没有语义差别。
    pub fn sessions(&self) -> SessionsHandle {
        SessionsHandle::new(self.state.clone())
    }

    pub async fn serve(self) -> io::Result<()> {
        axum::serve(self.listener, self.router).await
    }
}
