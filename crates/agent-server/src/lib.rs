//! session actor（issue 030）：把 ARCHITECTURE.md 关键判断 1 落地成代码——
//! `agent-store` 的 `Store` 是 `Rc<RefCell<Inner>>`，同步可重入是它的卖点，
//! 换成 `Arc<Mutex<_>>` 只会把重入变成死锁风险，所以**每个 session 独占一个
//! `std::thread`，store 活在里面**，外界只经 `mpsc<Command>` 进、
//! `broadcast<Frame>` 出（034 起是 `Frame`，agent 归属信封——`crate::event::
//! frame` 模块文档）。**只有这个 crate 知道线程和 tokio 的存在**——`agent-core`/
//! `agent-store` 永远是单线程视角，红线 7 也确实不辖这里（见根 `Cargo.toml`
//! 的仓库地图与 `docs/ARCHITECTURE.md`）。
//!
//! # 五个公开面
//!
//! | 类型 | 是什么 |
//! |---|---|
//! | [`Command`] | 外界唯一能对 session 说的话（`Input`/`Undo`/`Redo`/`Cancel`/`Shutdown`），032 生成 TS 类型的输入之一 |
//! | [`Frame`] | 034：`{ agent, event }` 信封，是 ARCHITECTURE.md §传输 下行 SSE 每一帧的真实形状 |
//! | [`SessionEvent`] | actor 广播的一切（`Frame.event` 那一半），`RunnerEvent` 的 owned、可序列化翻译 |
//! | [`SessionHandle`] | 一个 session 的把手：发命令、订阅事件、直接旁路取消 |
//! | [`SessionRegistry`] | `SessionId → SessionHandle`，`open`/`get`/`close`，M3 单副本内存表 |
//!
//! # 这个 crate 只驱动 `agent_runtime::run_turn`，不直接碰 `Session::step`
//!
//! 028 在并行改 `agent-core` 的公开面（多 agent 图，`Session::step` 的长 agent
//! 维度）。这个 crate 只经 [`agent_runtime::run_turn`] 与 `Session` 的公开命令
//! （`begin_turn`/`undo_turn`/`undo_turn_force`/`redo_turn`/读口）驱动，不直接
//! 调 `Session::step`——接缝留给 029 统一对齐（issue 030 原文）。
//!
//! # 崩溃隔离
//!
//! actor 线程 panic 不拖垮进程：[`actor`] 模块用 `catch_unwind` 接住，广播
//! [`SessionEvent::SessionDied`]，[`SessionRegistry`] 随后 `get`/`close` 报
//! `dead` 而不是静默移除——客户端要能问到死因。
//!
//! # HTTP/SSE 面（issue 031）
//!
//! [`http`] 模块把 axum 挂在上面这套 actor/handle/registry 之上：
//! [`AgentServer::new(config).serve(addr)`] 是唯一入口（ARCHITECTURE.md §传输
//! 原文，库形态不变，决策 12）。默认 `bind` 地址走 [`crate::bind`]
//! （`default_bind_addr`），红线 8 的「不许硬编码监听全部网卡」在那个模块结账。
//!
//! # 宿主装配（issue 035）
//!
//! [`bootstrap`] 把「读 `providers.toml` → 拼 [`SessionTemplate`]」这条各宿主
//! 重复的装配线收成一个函数——`agent-server-bin`、`examples/serve.rs` 都用它。
//! [`AgentServer::sessions`]/[`BoundAgentServer::sessions`] 给宿主一个优雅关闭
//! 的把手（[`SessionsHandle`]）：Ctrl-C 之类的退出信号发生时，枚举 + 关掉全部
//! 会话，落盘快照完整之后进程再退出。

mod actor;
mod bind;
mod bootstrap;
mod command;
mod event;
mod handle;
mod handle_compaction;
mod handle_remote_tools;
mod http;
mod provider_dispatch;
mod registry;
// 032：协议类型的 TS 生成，只在 `ts` feature 后面存在——见模块自己的文档注释。
#[cfg(feature = "ts")]
pub mod ts_protocol;

pub use actor::OpenError;
pub use bind::{
    AGENT_BIND_ENV, BindConfigError, default_bind_addr, default_bind_ip, resolve_bind_ip,
};
pub use bootstrap::{BootstrapError, BootstrapOptions, Bootstrapped, bootstrap};
pub use command::{Command, Granularity};
pub use event::{
    AutoTurnHold, BlockedCause, Frame, OrphanFate, SessionEvent, TransientSourceFailureCause,
    TransientSourceFailureEvent, UndoOutcome,
};
pub use handle::{SessionClosed, SessionHandle, Subscription};
pub use http::{AgentServer, BoundAgentServer, ServerConfig, SessionTemplate, SessionsHandle};
pub use provider_dispatch::resolve_provider;
pub use registry::{CloseError, OpenSpec, SessionId, SessionQuery, SessionRegistry, ToolTableSpec};
