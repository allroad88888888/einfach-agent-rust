//! [`ServerConfig`] / [`SessionTemplate`]：`AgentServer::new` 要的全部东西。
//!
//! `SessionTemplate` 是 [`crate::registry::OpenSpec`] 减去 `id`/`store_path`
//! 的那份——`POST /sessions` 只收 `{ "session_path": 可选 }`（issue 031 原文），
//! 其余开一个 session 要的东西（provider、endpoint、工具表……）是部署时定好的
//! 服务端配置，不是每次开会话客户端都要报一遍。
//!
//! # 035：`session_path` 没给时自动落盘（`default_sessions_dir`）
//!
//! 031 原文只定了「给了 `session_path` 就落盘，没给就是 `Memory`」——查
//! [`SessionRegistry::open`](crate::registry::SessionRegistry::open) 现状确认
//! 过，POST /sessions 不带 `session_path` 此前只有一条路：临时会话，进程退出
//! 即丢。`agent-server-bin` 的 `--sessions-dir <dir>` 语义（035 issue 原文）要求
//! 「不带 `session_path` 也落盘，自动分配 `<dir>/<id>.jsonl`」——这条自动分配
//! 逻辑不存在，最小补在这里：[`SessionTemplate::default_sessions_dir`] 是
//! `Some` 时，[`SessionTemplate::open_spec`] 在没收到显式 `session_path` 的情况
//! 下现造这个路径。宿主直接构造 `SessionTemplate { .. }` 或走
//! [`crate::bootstrap`] 都能设它；库层默认（`None`）保持 M3 以来的旧行为不变。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::SystemChunk;
use agent_providers::Provider;
use agent_transport::Client;

use crate::registry::{OpenSpec, SessionId, ToolTableSpec};

/// 031 原文点名的默认值：环形缓冲 256 帧、断开取消宽限期 5s。
pub const DEFAULT_RING_CAPACITY: usize = 256;
pub const DEFAULT_CANCEL_GRACE: Duration = Duration::from_secs(5);
pub const DEFAULT_SSE_KEEP_ALIVE: Duration = Duration::from_secs(15);

/// 开一个 session 要的全部东西，减去每次请求才知道的 `id`/`store_path`——
/// 两者由 [`Self::open_spec`] 在收到 `POST /sessions` 时补上。
#[derive(Clone)]
pub struct SessionTemplate {
    pub provider: Arc<dyn Provider>,
    pub endpoint: String,
    pub api_key: String,
    pub model: Arc<str>,
    pub tools: ToolTableSpec,
    /// 内置工具路径监狱的**根目录**——每个 session 实际锁在
    /// `tools_root/<session-id>/` 之内（[`Self::open_spec`] 现造，且现造前先
    /// `create_dir_all`；`ToolExecutor::new` 要求目录已存在才能
    /// canonicalize），互不踩脚。
    pub tools_root: PathBuf,
    pub system: Vec<SystemChunk>,
    pub client: Arc<Client>,
    pub history_cap: Option<usize>,
    pub snapshot_every: Option<u64>,
    pub provider_timeout: Option<Duration>,
    /// `None`（默认）→ 旧行为：`POST /sessions` 不带 `session_path` 就是
    /// `Memory`，进程退出即丢。`Some(dir)` → 不带 `session_path` 时自动落盘到
    /// `dir/<session-id>.jsonl`（[`Self::open_spec`]），`dir` 本身在那一刻现
    /// `create_dir_all`（跟 `tools_root` 同一个「调用方给的根目录不保证已存在」
    /// 取舍——不预先建好，`agent_runtime::jsonl::Jsonl` 的 IO 线程会在打不开
    /// 文件时静默吞错误，session 表面上还是 open 成功了，只是悄悄变成没人发现
    /// 的「其实没落盘」，这正是本模块「035」一节要堵的坑）。
    pub default_sessions_dir: Option<PathBuf>,
}

impl SessionTemplate {
    /// 补上 `id`/`store_path`，造一份可以直接喂给
    /// [`crate::registry::SessionRegistry::open`] 的 [`OpenSpec`]。
    ///
    /// `session_path`（客户端在 `POST /sessions` 请求体里显式给的）优先；没给
    /// 就落到 [`Self::default_sessions_dir`]（有就自动分配一个文件路径，没有
    /// 就还是 `Memory`——本文件模块文档「035」一节）。
    ///
    /// 唯一可能失败的地方是造目录——专属工具根目录，或者（自动分配路径这条分支
    /// 才会碰到的）`default_sessions_dir` 本身。`std::io::Error` 直接透传，路由
    /// 层负责翻成 500（这是宿主环境的磁盘/权限问题，不是客户端输入错误，不该
    /// 套用 `ApiError` 那几个 4xx 语义）。
    pub fn open_spec(&self, id: SessionId, session_path: Option<PathBuf>) -> std::io::Result<OpenSpec> {
        let tools_root = self.tools_root.join(id.as_str());
        std::fs::create_dir_all(&tools_root)?;
        let store_path = match session_path {
            Some(p) => Some(p),
            None => match &self.default_sessions_dir {
                Some(dir) => {
                    std::fs::create_dir_all(dir)?;
                    Some(dir.join(format!("{}.jsonl", id.as_str())))
                }
                None => None,
            },
        };
        Ok(OpenSpec {
            id,
            store_path,
            provider: Arc::clone(&self.provider),
            endpoint: self.endpoint.clone(),
            api_key: self.api_key.clone(),
            model: Arc::clone(&self.model),
            tools: self.tools,
            tools_root,
            system: self.system.clone(),
            client: Arc::clone(&self.client),
            history_cap: self.history_cap,
            snapshot_every: self.snapshot_every,
            provider_timeout: self.provider_timeout,
        })
    }
}

/// `AgentServer::new` 的输入。三个可调参数各自独立，`with_*` 是唯一的改法——
/// 字段本身不 `pub`,不然默认值就成了一句「除非你记得覆盖」的隐藏约定。
#[derive(Clone)]
pub struct ServerConfig {
    pub(crate) template: SessionTemplate,
    pub(crate) ring_capacity: usize,
    pub(crate) cancel_grace: Duration,
    pub(crate) sse_keep_alive: Duration,
    pub(crate) static_dir: Option<PathBuf>,
}

impl ServerConfig {
    pub fn new(template: SessionTemplate) -> Self {
        ServerConfig {
            template,
            ring_capacity: DEFAULT_RING_CAPACITY,
            cancel_grace: DEFAULT_CANCEL_GRACE,
            sse_keep_alive: DEFAULT_SSE_KEEP_ALIVE,
            static_dir: None,
        }
    }

    /// SSE 重连补发的环形缓冲能装多少帧（issue 031「默认 256 帧」）。
    pub fn with_ring_capacity(mut self, capacity: usize) -> Self {
        self.ring_capacity = capacity;
        self
    }

    /// 最后一个订阅者断开到旁路取消在飞轮次之间的宽限期（issue 031「默认
    /// 5s，可配」——测试要把它调短，不然「断言取消而非等到超时」的验收条目
    /// 就得等 5 秒一次）。
    pub fn with_cancel_grace(mut self, grace: Duration) -> Self {
        self.cancel_grace = grace;
        self
    }

    /// SSE 心跳（axum `KeepAlive` 注释行）的发送间隔。
    pub fn with_sse_keep_alive(mut self, interval: Duration) -> Self {
        self.sse_keep_alive = interval;
        self
    }

    /// issue 036：把 `dir`（`packages/web` 的构建产物，或桌面端打包进资源目录
    /// 的那份同一套 dist）从同一个端口发出去——同源零 CORS，本地缩影的正是
    /// 企业网关那种「前端由后端一并发出」的形态。不设就是纯 API 服务器（M3
    /// 之前的行为不变）。SPA 兜底 + API 路由优先的细节在
    /// [`crate::http::static_files`]。
    pub fn with_static_dir(mut self, dir: PathBuf) -> Self {
        self.static_dir = Some(dir);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_template(tools_root: PathBuf, default_sessions_dir: Option<PathBuf>) -> SessionTemplate {
        use agent_providers::deepseek::DeepSeek;
        use agent_transport::Client;

        use crate::registry::ToolTableSpec;

        SessionTemplate {
            provider: Arc::new(DeepSeek),
            endpoint: "http://127.0.0.1:1/unused".to_string(),
            api_key: "fake-key".to_string(),
            model: Arc::from("deepseek-v4-pro"),
            tools: ToolTableSpec::Builtin,
            tools_root,
            system: Vec::new(),
            client: Arc::new(Client::new()),
            history_cap: None,
            snapshot_every: None,
            provider_timeout: None,
            default_sessions_dir,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("agent-server-open-spec-test-{name}-{}", std::process::id()))
    }

    #[test]
    fn no_default_dir_and_no_explicit_path_stays_memory() {
        let template = minimal_template(temp_dir("tools-a"), None);
        let spec = template.open_spec(SessionId::from("s-1"), None).unwrap();
        assert_eq!(spec.store_path, None, "没有 default_sessions_dir，也没有显式 session_path，该还是 Memory");
    }

    #[test]
    fn explicit_session_path_wins_over_default_dir() {
        let default_dir = temp_dir("tools-b-default");
        let explicit = temp_dir("tools-b-explicit").join("custom.jsonl");
        let template = minimal_template(temp_dir("tools-b"), Some(default_dir));
        let spec = template.open_spec(SessionId::from("s-2"), Some(explicit.clone())).unwrap();
        assert_eq!(spec.store_path, Some(explicit), "客户端显式给的 session_path 该赢");
    }

    #[test]
    fn missing_session_path_auto_assigns_under_default_dir() {
        let dir = temp_dir("tools-c-sessions");
        let template = minimal_template(temp_dir("tools-c"), Some(dir.clone()));
        let spec = template.open_spec(SessionId::from("s-3"), None).unwrap();
        assert_eq!(spec.store_path, Some(dir.join("s-3.jsonl")), "该自动分配 <dir>/<id>.jsonl");
        assert!(dir.is_dir(), "default_sessions_dir 该被现造出来，不能指望 Jsonl 的 IO 线程默默失败");
    }
}
