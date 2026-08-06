//! [`RunnerCtx`]：一次会话期间不变的资源包，`run_turn` 借它执行 effect。
//!
//! 字段清单是 issue 012 定的：provider adapter、transport client、工具
//! executor、工具表、system chunks、`SessionConfig`、取消标志、`GuardHistory`
//! （第 3 层滚动窗口）、事件回调。`endpoint`/`api_key` issue 原文没点名，但
//! 没有它们打不了任何真实请求——补的位置跟旧版 `agent-cli::turn::TurnContext`
//! 一致（022/023 已经这么放，见那个文件的字段注释）。
//!
//! `provider`/`client` 包 `Arc` 不是 issue 原文写的 `Box`：`CallProvider` 的
//! IO 要能被超时检测**放弃**（不 join，见 `provider_call` 模块文档的事故
//! 记录），放弃之后那个 IO 线程要能带着自己那份引用继续跑到自然结束，
//! `Box` 做不到——这是本 issue 唯一一处偏离 issue 原文字面类型的地方。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use agent_core::{AgentId, AgentTree, ExecutionProfileId, Session, SessionConfig, SystemChunk};
use agent_mcp::McpRegistry;
use agent_providers::Provider;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::event::{AgentEvent, RunnerEvent};
use crate::execution_binding::{ExecutionBinding, GuardScope};
use crate::persist::SessionBackend;
use crate::tool_table::ToolTable;
/// 单次 `CallProvider` 允许占用的总时长，到点注入 `Event::Timeout`（012）。
pub const DEFAULT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(120);
/// 快照节奏默认值（027 决策 3）：每 10 个 turn 落一张。
pub const DEFAULT_SNAPSHOT_EVERY: u64 = 10;
/// 远端工具（`web:` / `desk:`）等待宿主回传的截止线（060）。**十分钟**。
///
/// 这个数不是「一次远端调用该花多久」的 UX 预算，而是**活性兜底**：它唯一的
/// 职责是保证会话不可能永久停在 `ToolsPending`。所以选值只按两侧代价算，不按
/// 「典型耗时」算：
///
/// - **误杀的代价高且发生在健康会话上**。`ask_user_question` 就在标准工具表里
///   （`ToolTable::standard`），它天生要等一个真人：读完问题、切个标签页、回来
///   作答，几分钟是常态。到点注入 `is_error` 会让模型对一个**正在正常等人**的
///   调用道歉/重问，用户看得见。
/// - **迟到的代价低且发生在已经坏掉的会话上**。真到了这条线，说明宿主永远不会
///   回传（前端崩了 / 网关挂了 / 客户端压根没实现这个工具），会话已经废了；
///   而且更快的逃生舱**本来就有**：用户 `POST /cancel`（立刻），以及 M9 的宽限
///   取消（最后一个订阅者断开 5s 后）已经覆盖「页面崩了/标签页关了」那一类。
///   这条线只兜最后一格——**客户端还连着，但永远不说话**。
///
/// 于是取「一个真人跟一次提问打交道绝不会超过、而人又绝不愿意再多等」的量级：
/// 10 分钟。同时它是 provider 超时（120s）的 5 倍——「人比模型慢」这件事写进
/// 数字里，而不是写成无限期。
///
/// **不按工具分类给不同默认**：位置与可逆性目前都由**名字**经自由函数推
/// （`tool_table::location_of` / `reversibility_of`），`ToolSpec` 里没有任何
/// per-tool 元数据的位置可挂；在这里现造一套分类会跟 050（工具名编码）撞车，
/// 也会抢掉 HOST-CAPABILITIES.md §四 的地盘——宿主声明自己的能力时把截止线
/// 一起带进来才是它的正位。现在给的是**一个宽松默认 + 一个可配置的口**
/// （[`RunnerCtx::with_remote_tool_timeout`]），等声明入口落地再细分。
pub const DEFAULT_REMOTE_TOOL_TIMEOUT: Duration = Duration::from_secs(600);

pub struct RunnerCtx {
    pub(crate) default_binding: ExecutionBinding,
    pub(crate) execution_bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
    pub(crate) default_guard_scope: GuardScope,
    pub(crate) execution_guard_scopes: BTreeMap<ExecutionProfileId, GuardScope>,
    pub(crate) next_guard_scope: u64,
    pub(crate) fs: ToolExecutor,
    pub(crate) tools: ToolTable,
    /// MCP server 的活句柄表（store 外的进程内 registry，红线 3）。dispatch 的第四路
    /// 只拿它 + server id 去查 client 起一次异步 `tools/call`（`crate::mcp_call`），
    /// client 句柄从不进任何 command/atom。默认空表——没接 MCP 的宿主永远查不到。
    pub(crate) mcp: Arc<McpRegistry>,
    pub(crate) system: Vec<SystemChunk>,
    pub(crate) cancel: Arc<AtomicBool>,
    /// 单次 MCP `tools/call` 的往返超时（`crate::mcp_call` 传给背景线程）。
    pub(crate) mcp_timeout: Duration,
    /// 远端工具等待宿主回传的截止线预算（060）。登记等待槽时按它算出
    /// `PendingRemoteTool::deadline`，之后这个槽的命运只看那个绝对时刻。
    pub(crate) remote_tool_timeout: Duration,
    pub(crate) guard_histories: BTreeMap<GuardScope, Vec<agent_core::cache::TurnHit>>,
    pub(crate) pending_remote_tools: crate::ctx_remote_tools::PendingRemoteTools,
    /// Reserved source inputs/results live only here.  This vault is process-local and has no
    /// serialization surface; durable core state contains policy placeholders only.
    pub(crate) transient_sources: crate::transient_source_vault::TransientSourceVault,
    pub(crate) image_resolver: Option<Arc<dyn crate::ImageResolver>>,
    pub(crate) image_preparation_failures:
        BTreeMap<AgentId, crate::image_preparation_failure::ImagePreparationFailure>,
    /// 011 的端口，027 上岗：`persist::sync` 每条命令之后转发进它，
    /// `persist::recover` 启动时从它读回。
    pub(crate) session_store: Box<SessionBackend>,
    /// `persist::sync` 的高水位：目前为止告诉过 `session_store` 的最大 `seq`
    /// （见该模块文档「为什么按 seq 高水位」）。
    pub(crate) persisted_seq: Option<u64>,
    /// 快照节奏，`0` = 关闭。
    pub(crate) snapshot_every: u64,
    /// 上一次落快照时的 `turn_id`，防止同一轮内被多次调用时重复落盘。
    pub(crate) last_snapshotted_turn: Option<u64>,
    /// 事件出口。**只有一条**：`new` 收的那条不带归属的回调在这里被包成带归属
    /// 的（丢掉 `agent`），[`RunnerCtx::with_agent_events`] 换掉整条。一个字段
    /// 而不是「普通回调 + 归属回调」两个字段，是因为两个字段就有「两条都设了
    /// 谁生效」这个必须回答、答什么都不好的问题。
    on_event: Box<dyn FnMut(AgentEvent)>,
    /// 048：树快照变化回调，**独立于 `on_event`**——树快照是整棵状态的投影，
    /// 不是 `RunnerEvent` 的第十个变体（那样会逼 `RunnerEvent` 的穷举 `match`
    /// 在 CLI print / io_thread / server `From` 三处连锁改，见 048 issue 范围
    /// 条款 1）。`None` = 没设（`with_tree_events` 也没调）——CLI 就是这个默认值，
    /// 它的 `/agents` 是按需读 `Session::agent_tree()`，不需要 pump 每步都算一遍。
    on_tree_change: Option<Box<dyn FnMut(AgentTree)>>,
    /// 072：远端等待槽的投影变化回调，跟 `on_tree_change` 同款独立字段。写点在
    /// **槽变化的那一刻**（`crate::ctx_remote_tools` 的四个变更点各通知一次），
    /// 不是宿主的命令边界——登记就在 `run_turn` 内部、下一行就广播
    /// `tool_executing` 了。设/不设与语义全在那个文件的模块文档里。
    pub(crate) on_pending_remote_tools:
        Option<Box<dyn FnMut(Vec<crate::ctx_remote_tools::RemoteToolWaiting>)>>,
    /// 092：认领协议的完整状态投影。它独立于旧版 waiting-only 投影，包含 revision、
    /// claim 归属与有界终态回执；服务端把它写进共享单元格，使只读状态查询不会排在
    /// actor 正在执行的 provider 网络调用之后。
    pub(crate) on_remote_tool_status:
        Option<Box<dyn FnMut(crate::remote_tool_protocol::RemoteToolStatusSnapshot)>>,
}

impl RunnerCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn Provider>,
        client: Arc<Client>,
        endpoint: String,
        api_key: String,
        fs: ToolExecutor,
        tools: ToolTable,
        system: Vec<SystemChunk>,
        session_config: SessionConfig,
        session_store: Box<SessionBackend>,
        mut on_event: Box<dyn FnMut(RunnerEvent)>,
    ) -> Self {
        let on_event: Box<dyn FnMut(AgentEvent)> =
            Box::new(move |ev: AgentEvent| on_event(ev.event));
        RunnerCtx {
            default_binding: ExecutionBinding::new(
                provider,
                client,
                endpoint,
                api_key,
                session_config,
            ),
            execution_bindings: BTreeMap::new(),
            default_guard_scope: GuardScope::INITIAL,
            execution_guard_scopes: BTreeMap::new(),
            next_guard_scope: GuardScope::FIRST_DYNAMIC,
            fs,
            tools,
            mcp: Arc::new(McpRegistry::new()),
            system,
            cancel: Arc::new(AtomicBool::new(false)),
            mcp_timeout: agent_mcp::DEFAULT_CALL_TIMEOUT,
            remote_tool_timeout: DEFAULT_REMOTE_TOOL_TIMEOUT,
            guard_histories: BTreeMap::new(),
            pending_remote_tools: crate::ctx_remote_tools::PendingRemoteTools::default(),
            transient_sources: crate::transient_source_vault::TransientSourceVault::default(),
            image_resolver: None,
            image_preparation_failures: BTreeMap::new(),
            session_store,
            persisted_seq: None,
            snapshot_every: DEFAULT_SNAPSHOT_EVERY,
            last_snapshotted_turn: None,
            on_event,
            on_tree_change: None,
            on_pending_remote_tools: None,
            on_remote_tool_status: None,
        }
    }

    /// 覆盖快照节奏（`0` = 关闭，只靠 entry 日志重放）。
    pub fn with_snapshot_every(mut self, every: u64) -> Self {
        self.snapshot_every = every;
        self
    }

    /// 换成带 agent 归属的事件回调（029）。**替换**，不是追加——见 `on_event`
    /// 字段注释。
    ///
    /// [`RunnerCtx::new`] 收的那条 `FnMut(RunnerEvent)` 是 M1..M2 的形状：单
    /// agent 时「谁说的」只有一个答案，问都不用问。多 agent 宿主（`agent-cli` 的
    /// 打印、将来的 SSE 协议面）换这一条，拿到的每件事都带着它出自哪个 agent。
    pub fn with_agent_events(mut self, on_event: Box<dyn FnMut(AgentEvent)>) -> Self {
        self.on_event = on_event;
        self
    }

    /// 设一条树快照变化回调（048）。`run_turn` 每次 `session.step` + persist 之后
    /// 重算 `Session::agent_tree()`，跟上一次算出来的比（`AgentTree: PartialEq`），
    /// **变了才调它**——见 `crate::runner` 模块里那个调用点的文档。
    ///
    /// 照 [`RunnerCtx::with_agent_events`] 同款：独立字段，**替换**不是追加。
    /// CLI 不设这一条——它的 `/agents` 是按需读 `agent_tree()`，没有必要为一条
    /// 从没接的回调让 `run_turn` 每步多算一次树（见 [`RunnerCtx::tree_events_enabled`]）。
    pub fn with_tree_events(mut self, on_tree_change: Box<dyn FnMut(AgentTree)>) -> Self {
        self.on_tree_change = Some(on_tree_change);
        self
    }

    /// 有没有设树快照回调——`run_turn` 用它决定要不要为这一步多算一次
    /// `agent_tree()`（048：没设就完全不算，`with_tree_events` 文档「CLI 不设」
    /// 那句话的落地，不是「算了但不发」）。
    pub(crate) fn tree_events_enabled(&self) -> bool {
        self.on_tree_change.is_some()
    }

    /// 树快照变了（由 `run_turn` 判断），把它发给宿主设的回调。没设就什么都
    /// 不做——`tree_events_enabled` 已经在调用点挡过一次，这里再挡一次纯粹是
    /// 防御性的（`Option::as_mut` 天然处理，没有 `unwrap` 需要担心的分支）。
    pub(crate) fn emit_tree(&mut self, tree: AgentTree) {
        if let Some(on_tree_change) = self.on_tree_change.as_mut() {
            on_tree_change(tree);
        }
    }

    /// pump 之外的路径也把当前树快照发出去（048 真机验收补漏）。
    ///
    /// `run_turn` 靠 pump 里的 change 检测 + [`RunnerCtx::emit_tree`] 发树；但
    /// undo / redo / 取消轮自动擦除走的是宿主的命令处理（**不经 `run_turn`**），
    /// 它们撤掉一棵子树之后同样得让活树面板 / `GET .../agents` 看到——否则
    /// 「undo 撤了子 agent，面板不动」（真机逮到的漏投影：core 层 `agent_tree()`
    /// 退了，SSE/GET 那一路没跟上）。宿主在这些命令之后调它一次。
    ///
    /// 没设 `with_tree_events`（CLI）就是 no-op，且不白算一次 `agent_tree()`
    /// （`tree_events_enabled` 先挡）。它无条件发（不做 change 检测）——调用点
    /// 已经知道「刚撤了一轮，树必然变了」，再比一次多余。
    pub fn emit_tree_snapshot(&mut self, session: &Session) {
        if self.tree_events_enabled() {
            self.emit_tree(session.agent_tree());
        }
    }

    /// 共享的取消标志：宿主的 Ctrl-C 处理器翻它。`run_turn` 内部只读它
    /// （写它的地方是 `CancelInFlight` effect 的执行点，见 `runner` 模块）。
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// 宿主装载的全部 skill 的 (id, 描述)，按 id 排序（039）。CLI 的 `/skills`
    /// 列表用它——「有哪些可用」是 registry 的事，「哪些激活了」问 `Session`。
    pub fn available_skills(&self) -> Vec<(Arc<str>, Arc<str>)> {
        self.tools.skill_registry().listing()
    }

    /// 装上宿主持有的 [`McpRegistry`]（store 外的活句柄表，红线 3）。默认是空表——
    /// 没配 MCP server 的宿主（CLI 尚未接 044/045、浏览器 host 只有 http）dispatch
    /// 查不到任何 server，`mcp:` 工具压根不会进工具表，这个空表也就永远不被查到。
    pub fn with_mcp(mut self, registry: Arc<McpRegistry>) -> Self {
        self.mcp = registry;
        self
    }

    /// 覆盖 MCP `tools/call` 的往返超时——测试用短超时把「server 挂住不回」压到
    /// 毫秒级（跟 [`RunnerCtx::with_provider_timeout`] 同款）。
    pub fn with_mcp_timeout(mut self, timeout: Duration) -> Self {
        self.mcp_timeout = timeout;
        self
    }

    /// 覆盖远端工具等待宿主回传的截止线（060，默认
    /// [`DEFAULT_REMOTE_TOOL_TIMEOUT`] = 10 分钟）。
    ///
    /// 跟 [`RunnerCtx::with_provider_timeout`] / [`RunnerCtx::with_mcp_timeout`]
    /// 同款：测试把「客户端永不回传」压到毫秒级；宿主也可以按自己的交互形态调
    /// （纯机器执行的注入能力可以调短，含真人问答的该留够）。
    ///
    /// **只影响此后新登记的等待槽**：已经在等的槽握的是登记那一刻算好的绝对
    /// 时刻，不会被中途改配置追溯。
    pub fn with_remote_tool_timeout(mut self, timeout: Duration) -> Self {
        self.remote_tool_timeout = timeout;
        self
    }

    /// 发一件事给宿主，带上它出自哪个 agent。
    ///
    /// **归属由调用点提供，不由 `RunnerCtx` 猜**：runner 每一处 `emit` 都正好
    /// 知道自己在替谁做事（effect 的 `agent` 字段、`step` 那条事件的 `agent`、
    /// IO 线程自己那份 tag），存一个「当前 agent」字段反而会在并行的子 agent 之间
    /// 串味——那正是这个 issue 要解决的问题，不是解决它的手段。
    pub(crate) fn emit(&mut self, agent: &AgentId, event: RunnerEvent) {
        (self.on_event)(AgentEvent {
            agent: agent.clone(),
            event,
        });
    }
}

#[cfg(test)]
#[path = "ctx_tests.rs"]
mod tests;
