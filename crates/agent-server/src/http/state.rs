//! [`AppState`]：所有路由处理函数共享的东西——`axum` 的 `State` extractor 要求
//! 它 `Clone`，于是真正的数据全部包在一个 `Arc` 里（`Inner`），`AppState` 本身
//! 只是一个薄句柄。
//!
//! 三件事：造新 session id（[`AppState::generate_id`]）、查一个 id 现在活着
//! 还是死了并给出统一的 [`ApiError`]（[`AppState::session_handle`]）、拿到
//! （必要时现造）这个 session 的 SSE hub（[`AppState::hub_for`]）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::http::config::{ServerConfig, SessionTemplate};
use crate::http::error::ApiError;
use crate::http::hub::SseHub;
use crate::registry::{SessionId, SessionQuery, SessionRegistry};
use crate::{Command, SessionHandle};

#[derive(Clone)]
pub(crate) struct AppState(Arc<Inner>);

struct Inner {
    registry: SessionRegistry,
    template: SessionTemplate,
    hubs: Arc<Mutex<HashMap<SessionId, Arc<SseHub>>>>,
    ring_capacity: usize,
    cancel_grace: Duration,
    sse_keep_alive: Duration,
    /// session id 生成器：`sess-<进程 pid>-<单调计数器>`。够用（M3 单副本、
    /// 进程内单调），不为此拉一个 uuid 依赖——见 `crate::http::state` 模块
    /// 文档「三件事」第一件。
    next_id: AtomicU64,
}

impl AppState {
    pub(crate) fn new(config: ServerConfig) -> Self {
        AppState(Arc::new(Inner {
            registry: SessionRegistry::new(),
            template: config.template,
            hubs: Arc::new(Mutex::new(HashMap::new())),
            ring_capacity: config.ring_capacity,
            cancel_grace: config.cancel_grace,
            sse_keep_alive: config.sse_keep_alive,
            next_id: AtomicU64::new(0),
        }))
    }

    pub(crate) fn generate_id(&self) -> SessionId {
        let n = self.0.next_id.fetch_add(1, Ordering::Relaxed);
        SessionId::from(format!("sess-{}-{n}", std::process::id()))
    }

    pub(crate) fn template(&self) -> &SessionTemplate {
        &self.0.template
    }

    pub(crate) fn registry(&self) -> &SessionRegistry {
        &self.0.registry
    }

    pub(crate) fn sse_keep_alive(&self) -> Duration {
        self.0.sse_keep_alive
    }

    /// 查一个 id：活着就给句柄,`None`/死了就给对应的 [`ApiError`]
    /// （404/410——issue 031「404/409/410（dead）分明」）。所有需要「先确认
    /// session 还活着」的路由（input/undo/redo/cancel）共用这一次判断。
    pub(crate) fn session_handle(&self, id: &SessionId) -> Result<SessionHandle, ApiError> {
        match self.0.registry.get(id) {
            None => Err(ApiError::not_found(format!(
                "session \"{id}\" 不存在（从没 open 过，或者已经被 close 摘表）"
            ))),
            Some(SessionQuery::Dead { reason }) => Err(ApiError::gone(reason)),
            Some(SessionQuery::Alive(handle)) => Ok(handle),
        }
    }

    /// `input`/`undo`/`redo`/`cancel` 四个命令端点共用的路径：查活着的句柄、
    /// 发命令。`SessionClosed`（`session_handle` 确认活着之后、`send` 真正执行
    /// 之前这极窄的窗口里 actor 死了）跟 [`Self::session_handle`] 报的 410 用
    /// 同一个错误码——客户端看到的是同一种情况：这条命令没能送到一个活的
    /// session。
    pub(crate) fn dispatch(&self, id: &SessionId, cmd: Command) -> Result<(), ApiError> {
        let handle = self.session_handle(id)?;
        handle.send(cmd).map_err(|_| {
            ApiError::gone("session 的 actor 线程在这条命令送达之前已经不在了".to_string())
        })
    }

    /// 拿到这个 session 的 SSE hub，没有就现造一个（`hub_for` 是唯一入口——
    /// [`SseHub`] 不暴露公开构造函数）。造之前先确认 session 活着：一个从没
    /// `open` 过或者已经死了的 id 不该凭空长出一个 hub 和后台任务。
    pub(crate) fn hub_for(&self, id: &SessionId) -> Result<Arc<SseHub>, ApiError> {
        if let Some(hub) = self.0.hubs.lock().unwrap().get(id) {
            return Ok(Arc::clone(hub));
        }
        let handle = self.session_handle(id)?;
        let mut hubs = self.0.hubs.lock().unwrap();
        // 双检：拿锁之间可能有另一个并发请求已经把它造好了。
        if let Some(hub) = hubs.get(id) {
            return Ok(Arc::clone(hub));
        }
        let hub = SseHub::spawn(
            handle,
            self.0.ring_capacity,
            self.0.cancel_grace,
            id.clone(),
            Arc::clone(&self.0.hubs),
        );
        // ↑ `self.0.hubs` 本身已经在 `Arc<Inner>` 里了，这里又包一层 `Arc` 是
        // 故意的：`SseHub::spawn` 只想要「一份能长期持有、不依赖 `AppState`
        // 生死」的表引用（自清理任务活得可能比某一次请求的 `AppState` clone 长
        // ——虽然实践中 `AppState` 本身也是 `Arc`，这里保持 `hub` 模块不对
        // `AppState`/`Inner` 的存在有任何假设，`guard.rs` 的单元测试也是直接
        // 造一个独立的 `Arc<Mutex<HashMap<..>>>` 喂给 `SseHub::spawn`）。
        hubs.insert(id.clone(), Arc::clone(&hub));
        Ok(hub)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Instant;

    use agent_providers::deepseek::DeepSeek;
    use agent_transport::Client;

    use crate::http::config::SessionTemplate;
    use crate::registry::ToolTableSpec;

    /// 不连任何上游的模板：这条测试只开/关 session，一次 `Input` 都不发，
    /// `endpoint` 永远不会被真的拨号（跟 `tests/http_sessions_handle_closes_
    /// all_open_sessions.rs` 用 `http://127.0.0.1:1/unused` 同一个取舍）。
    fn template() -> SessionTemplate {
        SessionTemplate {
            provider: Arc::new(DeepSeek),
            endpoint: "http://127.0.0.1:1/unused".to_string(),
            api_key: "fake-key".to_string(),
            model: Arc::from("deepseek-v4-pro"),
            tools: ToolTableSpec::Builtin,
            tools_root: std::env::temp_dir()
                .join(format!("agent-server-hub-reclaim-{}", std::process::id())),
            system: Vec::new(),
            client: Arc::new(Client::new()),
            history_cap: None,
            snapshot_every: None,
            provider_timeout: None,
            remote_tool_timeout: None,
            default_sessions_dir: None,
        }
    }

    fn hub_ids(state: &AppState) -> Vec<SessionId> {
        let mut ids: Vec<SessionId> = state.0.hubs.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// issue 059：session 死了，它的 hub 必须从这张表里**摘掉**——不是「少一点」，
    /// 是精确归零。清理动作只有一处（`SseHub::spawn` 的 drain 任务跑完
    /// `sub.recv()` 之后那句 `hubs.remove`），所以这条测试同时钉住两件事：
    /// drain 任务真的会在 session 死后退出，且退出时真的摘表。
    ///
    /// 关的路径是 `SessionRegistry::close`——`SessionsHandle::close_all`
    /// （宿主 Ctrl-C 优雅退出）和 `tests/close_then_reopen_recovers.rs` 走的
    /// 都是它，这里不另造一条关闭路。
    #[tokio::test(flavor = "multi_thread")]
    async fn closing_every_session_empties_the_hub_table() {
        let state = AppState::new(ServerConfig::new(template()));

        let mut ids = Vec::new();
        for _ in 0..3 {
            let id = state.generate_id();
            let spec = state
                .template()
                .open_spec(id.clone(), None, Vec::new(), Vec::new(), Vec::new())
                .expect("工具根目录该建得起来");
            state
                .registry()
                .open(spec)
                .expect("开一个干净的新 session 不该失败");
            // 跟 `POST /sessions` 同一条路：开完立刻现造 hub，然后把这次请求
            // 拿到的那份 `Arc` 丢掉（路由层也是 `let _ = state.hub_for(&id)`）。
            drop(state.hub_for(&id).expect("刚 open 成功，hub 该造得出来"));
            ids.push(id);
        }
        assert_eq!(
            hub_ids(&state).len(),
            3,
            "三个 session 各自该有一个 hub 挂在表上"
        );

        for id in &ids {
            state
                .registry()
                .close(id)
                .expect("三个都是干净的活会话，优雅关闭不该报错");
        }

        // drain 任务的退出是异步的（它得先被调度到、发现广播端没了），给它一段
        // 远超实际需要的窗口——真修好了通常几毫秒内就空了，这里的上限只是为了
        // 在没修好时给出一个确定的失败，而不是挂死。
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !hub_ids(&state).is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            hub_ids(&state),
            Vec::<SessionId>::new(),
            "全部 close 之后 hub 表该精确归零（issue 059）"
        );
    }
}
