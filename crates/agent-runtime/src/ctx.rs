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

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use agent_core::cache::TurnHit;
use agent_core::{AgentId, AgentTree, Session, SessionConfig, SystemChunk};
use agent_providers::Provider;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::event::{AgentEvent, RunnerEvent};
use crate::persist::SessionBackend;
use crate::tool_table::ToolTable;
/// 单次 `CallProvider` 允许占用的总时长，到点注入 `Event::Timeout`（012）。
pub const DEFAULT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(120);
/// 快照节奏默认值（027 决策 3）：每 10 个 turn 落一张。
pub const DEFAULT_SNAPSHOT_EVERY: u64 = 10;

pub struct RunnerCtx {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) client: Arc<Client>,
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
    pub(crate) fs: ToolExecutor,
    pub(crate) tools: ToolTable,
    pub(crate) system: Vec<SystemChunk>,
    pub(crate) session_config: SessionConfig,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) provider_timeout: Duration,
    pub(crate) guard_history: Vec<TurnHit>,
    pub(crate) pending_remote_tools: crate::ctx_remote_tools::PendingRemoteTools,
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
        let on_event: Box<dyn FnMut(AgentEvent)> = Box::new(move |ev: AgentEvent| on_event(ev.event));
        RunnerCtx {
            provider,
            client,
            endpoint,
            api_key,
            fs,
            tools,
            system,
            session_config,
            cancel: Arc::new(AtomicBool::new(false)),
            provider_timeout: DEFAULT_PROVIDER_TIMEOUT,
            guard_history: Vec::new(),
            pending_remote_tools: crate::ctx_remote_tools::PendingRemoteTools::default(),
            session_store,
            persisted_seq: None,
            snapshot_every: DEFAULT_SNAPSHOT_EVERY,
            last_snapshotted_turn: None,
            on_event,
            on_tree_change: None,
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

    /// 覆盖默认的 120s 超时——测试用短超时把「挂住不回」的场景压到毫秒级。
    pub fn with_provider_timeout(mut self, timeout: Duration) -> Self {
        self.provider_timeout = timeout;
        self
    }

    /// 运行时切 provider（014 `/model <name>`）：换 adapter + endpoint + key +
    /// model，并清空第 3 层滚动窗口——`guard_history` 记的是「最近几轮的缓存
    /// 命中观测」，换家之后旧家的观测对新家的命中率毫无意义，留着只会把两家
    /// 完全不相关的数字拼进同一条趋势线，误导 [`crate::guard`] 的滚动窗口判读。
    ///
    /// **不碰的东西，都是刻意的**：
    /// - 消息历史不在这里——`RunnerCtx` 根本不持有它（027 起活在
    ///   `Session::messages()`），历史保留是宿主（`agent-cli`）自己决定的事，
    ///   跨家续聊是合法场景。
    /// - 前缀镜像（第 1 层比对用）同理不在这里——027 起活在
    ///   `Session::prev_prefix()`，调用方必须自己清掉（`agent_cli::model_switch`
    ///   调 `Session::clear_prev_prefix()`；不清的话第 1 层会拿新家的请求去对
    ///   旧家的镜像，把正常的家族切换误判成前缀漂移）。
    pub fn switch_provider(&mut self, provider: Arc<dyn Provider>, endpoint: String, api_key: String, model: Arc<str>) {
        self.provider = provider;
        self.endpoint = endpoint;
        self.api_key = api_key;
        self.session_config.model = model;
        self.guard_history.clear();
    }

    /// 发一件事给宿主，带上它出自哪个 agent。
    ///
    /// **归属由调用点提供，不由 `RunnerCtx` 猜**：runner 每一处 `emit` 都正好
    /// 知道自己在替谁做事（effect 的 `agent` 字段、`step` 那条事件的 `agent`、
    /// IO 线程自己那份 tag），存一个「当前 agent」字段反而会在并行的子 agent 之间
    /// 串味——那正是这个 issue 要解决的问题，不是解决它的手段。
    pub(crate) fn emit(&mut self, agent: &AgentId, event: RunnerEvent) {
        (self.on_event)(AgentEvent { agent: agent.clone(), event });
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::TokenUsage;
    use agent_core::cache::TurnHit;
    use agent_providers::deepseek::DeepSeek;
    use agent_providers::kimi::Kimi;

    use crate::tool_table::ToolTable;

    fn build(model: &str) -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        RunnerCtx::new(
            Arc::new(DeepSeek),
            Arc::new(Client::new()),
            "https://api.deepseek.com/chat/completions".to_string(),
            "deepseek-key".to_string(),
            fs,
            ToolTable::builtin(),
            Vec::new(),
            SessionConfig {
                model: Arc::from(model),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
            crate::persist::open_backend(None, |_| {}),
            Box::new(|_| {}),
        )
    }

    #[test]
    fn switch_provider_replaces_adapter_endpoint_key_model_and_clears_guard_window() {
        let mut ctx = build("deepseek-v4-pro");
        ctx.guard_history.push(TurnHit::from_usage(&TokenUsage { prompt: 100, completion: 10, cached: Some(64) }));
        assert!(!ctx.guard_history.is_empty());

        ctx.switch_provider(
            Arc::new(Kimi),
            "https://api.moonshot.cn/v1/chat/completions".to_string(),
            "kimi-key".to_string(),
            Arc::from("kimi-k3"),
        );

        assert_eq!(ctx.endpoint, "https://api.moonshot.cn/v1/chat/completions");
        assert_eq!(ctx.api_key, "kimi-key");
        assert_eq!(&*ctx.session_config.model, "kimi-k3");
        assert!(ctx.guard_history.is_empty(), "跨家滚动窗口该清空，不能把 deepseek 的观测带进 kimi 的命中率");
    }

    /// 014 验收原文点名的断言：切到 kimi 之后，真的 `encode` 一次，产出的
    /// body 得是 kimi 的形状——带上新 model 名、不残留旧家的 model 名。只测
    /// `switch_provider` 换掉的三个字段（`provider`/`endpoint`/`session_config.
    /// model`）互相独立地对不上是不够的：万一 `provider` 换了但
    /// `session_config.model` 没跟着换（或者反过来），字段级断言会各自通过，
    /// 只有真的 encode 一次才会暴露「adapter 用的是新家，却拿旧家的 model
    /// 名去发请求」这种组合错误。
    #[test]
    fn switch_provider_encode_reflects_the_new_family_not_the_old() {
        let mut ctx = build("deepseek-v4-pro");
        ctx.switch_provider(
            Arc::new(Kimi),
            "https://api.moonshot.cn/v1/chat/completions".to_string(),
            "kimi-key".to_string(),
            Arc::from("kimi-k3"),
        );

        let encoded = ctx.provider.encode(&agent_providers::Ingredients {
            system: &[],
            messages: &[],
            tools: &[],
            late_tools: &[],
            late_system: &[],
            config: &ctx.session_config,
            intent: agent_core::RequestIntent::Free,
            prev_prefix: None,
        });

        let body = String::from_utf8(encoded.body).unwrap();
        assert!(body.contains("kimi-k3"), "encode 该带上切换后的 model 名: {body}");
        assert!(!body.contains("deepseek-v4-pro"), "encode 出的 body 不该残留切换前那家的 model 名: {body}");
    }
}
