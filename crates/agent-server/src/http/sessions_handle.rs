//! [`SessionsHandle`]（035）：宿主（`agent-server-bin`、桌面内嵌）优雅关闭时
//! 用的把手——枚举当前登记的 session id、逐个 `close`（发 `Shutdown`，排在各自
//! 队列末尾，等 actor 线程把手头的活处理完再退出、`join` 线程——不是 `Cancel`
//! 那种旁路打断）。`close` 返回之前，底层 `agent_runtime::jsonl::Jsonl` 的 IO
//! 线程已经把所有排队写入落盘（`Jsonl` 的 `Drop` 会 `join` 那条线程，
//! `SessionRegistry::close` 文档「摘表在 join 之前」保证了这一点）——这正是
//! Ctrl-C 优雅退出要的效果：会话文件在进程真正退出之前完整落盘。
//!
//! # 为什么不直接把 `SessionRegistry` 公开出去
//!
//! `SessionRegistry::open` 是 `POST /sessions` 路由内部用的——它跳过
//! `SessionTemplate::open_spec` 现造 `tools_root`/`default_sessions_dir` 那段
//! 逻辑，直接开一个宿主自己拼的 `OpenSpec` 不是这个类型该允许外界做的事。
//! `SessionsHandle` 只裁「关」这一半：[`Self::ids`] 只读、[`Self::close_all`]
//! 只调用既有的 `close`，不给「开」的能力。
//!
//! # 为什么是 `AppState` 的一层克隆，不是新起一张表
//!
//! `AppState` 本身已经是 `Arc<Inner>` 的薄句柄——克隆它不复制 registry，只是
//! 多一个指向同一份数据的引用，因此这里拿到的 registry 跟 axum 路由背后真正
//! 处理 HTTP 请求的是同一份，不是另开一张影子表。[`crate::AgentServer::
//! sessions`]/[`crate::BoundAgentServer::sessions`] 都只借 `&self`，在
//! `bind`/`serve` 消费掉 `self` 之前调用。

use crate::registry::{CloseError, SessionId};

use super::state::AppState;

#[derive(Clone)]
pub struct SessionsHandle(AppState);

impl SessionsHandle {
    pub(super) fn new(state: AppState) -> Self {
        SessionsHandle(state)
    }

    /// 当前登记表里的全部 id——含已经死掉但还没被 `close` 摘表的（`crate::
    /// registry::SessionRegistry::ids` 文档：这里刻意不区分死活，「关掉能关的」
    /// 不需要先问一遍谁还活着）。
    pub fn ids(&self) -> Vec<SessionId> {
        self.0.registry().ids()
    }

    /// 逐个 `close`：Shutdown 排进各自队列末尾、等 actor 线程处理完手头的活
    /// 再退出、`join` 线程——保证返回时全部会话已经落盘完整可恢复。**阻塞**
    /// （底层 `join` 是同步调用）——在异步上下文里用记得
    /// `tokio::task::spawn_blocking`，或者接受这一小段阻塞发生在「马上要退出
    /// 进程」的收尾路径上（`agent-server-bin` 的 Ctrl-C 处理走的是后者）。
    /// 单个 session 的 close 失败不阻止继续关别的——尽量关掉能关的，不是
    /// 「一个失败就放弃剩下的」，失败原因随每个 id 一起报回去，宿主决定要不要
    /// 打印。
    pub fn close_all(&self) -> Vec<(SessionId, Result<(), CloseError>)> {
        self.ids()
            .into_iter()
            .map(|id| {
                let outcome = self.0.close_session(&id);
                (id, outcome)
            })
            .collect()
    }
}
