//! [`OpenSpec`]：开一个 session 要的全部东西。
//!
//! **全部字段 `Send + 'static`**——这份配置本身要整体 `move` 进 actor 的
//! `std::thread::spawn` 闭包（`crate::actor` 模块文档），`Session`/`RunnerCtx`
//! 本身（`!Send`，含 `Rc<RefCell<_>>`）绝不允许出现在这个类型里，只能在线程
//! 内部用这份配置现造。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{AgentLimits, SystemChunk};
use agent_providers::Provider;
use agent_transport::Client;

use super::SessionId;

/// 开一个 session 要的全部东西。
///
/// `provider` 直接收 `Arc<dyn Provider>`，不是名字字符串——production 代码用
/// [`crate::provider_dispatch::resolve_provider`] 按名字查表拿到它（那张表照抄
/// `agent_cli::provider::build_provider` 的手法，issue 030 原文点名的「配置
/// 驱动三家分发」），测试可以直接喂一个自造的假 `Provider`（比如崩溃隔离测试
/// 要用到的那种），两者共用同一条 `OpenSpec` 构造路径,不必为「生产」和「测试」
/// 分叉出两个 `open` 入口。
pub struct OpenSpec {
    pub id: SessionId,
    /// `Some` → `Jsonl`（真文件，跟 CLI `--session <path>` 同款语义：有路径就
    /// 落盘，没有就是临时会话）；`None` → `Memory`（actor 关闭/进程退出即丢）。
    pub store_path: Option<PathBuf>,
    pub provider: Arc<dyn Provider>,
    pub endpoint: String,
    pub api_key: String,
    pub model: Arc<str>,
    pub tools: ToolTableSpec,
    /// 内置工具的路径监狱根——跟 `agent-cli` 一样锁在这棵目录树之内
    /// （`ToolExecutor::new` 的既有语义，这里不改）。
    pub tools_root: PathBuf,
    pub system: Vec<SystemChunk>,
    pub client: Arc<Client>,
    /// `None` → `agent_core::DEFAULT_HISTORY_CAP`。
    pub history_cap: Option<usize>,
    /// `None` → `agent_runtime` 的默认快照节奏（每 10 个 turn）。
    pub snapshot_every: Option<u64>,
    /// `None` → `agent_runtime` 的默认 provider 超时（120s）。
    pub provider_timeout: Option<Duration>,
}

/// 工具表的三档，跟 `agent_runtime::ToolTable::builtin`/`with_shell`/
/// `with_spawn` 一一对应——`OpenSpec` 不直接收一个建好的 `ToolTable`：那个类型
/// 不是 `Clone`，而 `spawn` 失败重试、未来配置热更新一类场景要求这份配置本身
/// 可以廉价复制。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolTableSpec {
    /// 013 的内置只读集：`srv:fs/read`、`srv:fs/list`。
    Builtin,
    /// 内置只读集 + `srv:shell/exec`（020/027 开闸）。
    WithShell,
    /// 034 开闸：内置只读集 + `srv:shell/exec` + `srv:agent/spawn`
    /// （029 给 runtime 的多 agent 能力，经这一档接满到 HTTP 协议面）。
    /// `spawn_limits` 走 server 配置（`SessionTemplate::tools`——`ServerConfig`
    /// 的一部分），默认 = [`AgentLimits::default`]（029 的默认）；`examples/
    /// serve.rs` 用这一档开满档。**这里的数字必须跟 `Session` 手上那份是同一组
    /// 数**——`agent_runtime::ToolTable::with_spawn` 文档记了这个耦合，工具描述
    /// 里告诉模型的数字得跟真正拦人的两道闸一致。`crate::actor::body` 在新建会话
    /// 时调 [`Session::set_agent_limits`](agent_core::Session::set_agent_limits)
    /// 对齐这组数（走 [`spawn_limits`](ToolTableSpec::spawn_limits) 这个读口），
    /// 不是留给调用方自己记得两边传同一个值。
    Full { spawn_limits: AgentLimits },
}

impl ToolTableSpec {
    pub(crate) fn build(self) -> agent_runtime::ToolTable {
        match self {
            ToolTableSpec::Builtin => agent_runtime::ToolTable::builtin(),
            ToolTableSpec::WithShell => agent_runtime::ToolTable::with_shell(),
            ToolTableSpec::Full { spawn_limits } => agent_runtime::ToolTable::with_shell().with_spawn(spawn_limits),
        }
    }

    /// `Full` 档带的 spawn 上限，供 `crate::actor::body` 在新建（非恢复）
    /// `Session` 时调 `Session::set_agent_limits` 对齐——两边不能只改工具表这
    /// 一侧的数字（本类型文档），这个读口是让「对齐」从一条纪律变成一次函数
    /// 调用。
    pub(crate) fn spawn_limits(self) -> Option<AgentLimits> {
        match self {
            ToolTableSpec::Full { spawn_limits } => Some(spawn_limits),
            ToolTableSpec::Builtin | ToolTableSpec::WithShell => None,
        }
    }
}
