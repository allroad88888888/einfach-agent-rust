//! 一次「开会话」的装配线：IndexedDB → `SessionStore` → 恢复 → [`RunnerCtx`]。
//!
//! 这是 `agent-cli::main` 那段装配在浏览器里的对应物，**逐步对齐**（顺序、
//! `seed_after_recover` 的必调、恢复出来卡在非终态时的处理，一条都不省）。
//! 差别只有三处外部输入换了来源：
//!
//! | | CLI | 浏览器 |
//! |---|---|---|
//! | provider 配置 | `providers.toml` | 页面传进来的 JSON（[`crate::config`]） |
//! | 会话落点 | `Jsonl`（文件） | `WebIdbStore`（IndexedDB，114a + 114c） |
//! | 工具 executor | `ToolExecutor`（真文件系统） | `NullToolExecutor`（112 的注入接缝） |
//!
//! 工具表见 [`crate::tools`]：空表起步 + 三条内建 `web:` 声明 + 页面自己声明的
//! 那一段，以及声明 skill 时的 `srv:skill/read`/session-start 索引；没有 `mcp:`、
//! 不开 spawn/status/collect。
//! **每一档 `with_*` 都是一次独立授权**——浏览器宿主目前一档都不开，所以 prompt
//! 最前面那段字节就是那几条声明。

use std::rc::Rc;
use std::sync::Arc;

use agent_core::{AgentId, AgentValue, AtomKey, Session, SessionConfig, SystemChunk};
use agent_runtime::persist::idb::{IdbDatabaseKv, WebIdbStore};
use agent_runtime::{PersistedMeta, RunnerCtx, SessionBackend};
use agent_tools::NullToolExecutor;
use agent_transport::Client;

use crate::config::HostConfig;
use crate::db;

/// 常驻 system 前缀。**固定字面量**：红线 11 禁止把时间戳、请求 id、随机 id
/// 写进 system prompt——那会让每一轮都是全新前缀、每一轮都全价。
///
/// **写成英文是有原因的**（2026-08-13 改）：这一份 wasm 宿主同时是对外 demo，
/// 而 demo 的受众是英文社区（决策 165 L1）。原来是中文，于是**英文提问会得到
/// 中文回答**——录 GIF 时才发现。页面全英文而 agent 说中文，比任何一处标签
/// 没译都更伤，因为它看起来不像「没顾上」，像「这东西不是给你用的」。
///
/// 最后一句是有意加的：模型答什么语言由用户那句话决定，而不是由这份提示词
/// 的语言决定——这样中文用户提中文问题，仍然得到中文回答。
const BASE_SYSTEM: &str = "You are a concise, honest assistant running inside the user's browser. When you need information about the page itself (title, address), call the matching tool instead of guessing. Reply in the language the user writes in.";

/// 人工参与的页面工具（提问、上传、确认）可以等到 10 分钟；取消仍走既有即时
/// 信号，不因这条预算变慢。直接使用 runtime 默认值，避免两个宿主的约定漂移。
const HOST_TOOL_TIMEOUT: std::time::Duration = agent_runtime::ctx::DEFAULT_REMOTE_TOOL_TIMEOUT;

/// 装配好的一个活会话。`Session` 与 `RunnerCtx` 一起活、一起换——「切会话」就是
/// 整个换掉这个结构体。
pub(crate) struct Live {
    pub(crate) id: String,
    pub(crate) session: Session,
    pub(crate) ctx: RunnerCtx,
}

/// 开一个会话。`on_event` 是 runner 事件的出口（带 agent 归属），`on_store_error`
/// 是持久化后端的错误出口——两条都由调用方给，这个函数不替它们决定送去哪。
pub(crate) async fn open(
    id: String,
    config: &HostConfig,
    on_event: Box<dyn FnMut(agent_runtime::AgentEvent)>,
    on_store_error: impl Fn(String) + 'static,
) -> Result<Live, String> {
    // 两个地方要用它（后端自己的错误出口，以及下面恢复期 fail-close 的失败），
    // 而它是 `impl Fn` 只能 move 一次——包一层 `Rc` 就够，这个宿主是单线程的。
    let on_store_error = Rc::new(on_store_error);
    let report_store_error = Rc::clone(&on_store_error);
    let database = db::open(&id).await?;
    let kv = IdbDatabaseKv::new(database, db::OBJECT_STORE);
    // 这一次 `await` 就是「从自己的 journal 忠实重放」（决策 6）真正发生的地方，
    // 也是 114a 那层薄绑定第一次真跑 IndexedDB 事务。之后 `SessionStore::load()`
    // 读的是它维护的 mirror，见 `web_store.rs` 模块文档。
    let store: WebIdbStore<AtomKey, AgentValue, PersistedMeta, IdbDatabaseKv> =
        WebIdbStore::open(kv, move |error| on_store_error(error.to_string())).await;
    let store: Box<SessionBackend> = Box::new(store);

    let (mut session, restored) = match agent_runtime::recover(
        store.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        // 快照里有这一版不认识的键：忽略并继续，跟 CLI 同一条处理。
        &mut |_key| {},
    ) {
        Ok(Some(recovered)) => (recovered, true),
        Ok(None) => (Session::new(AgentId::root()), false),
        // `Refused` = journal 读不回来。**硬失败**，不静默当成新会话——下一张
        // 快照就会把现场覆盖掉（`LoadOutcome` 文档里那条真 bug）。
        Err(error) => return Err(format!("会话恢复失败：{error}")),
    };
    let needs_fail_close = agent_runtime::recovered_transient_source_needs_fail_close(&session);

    let provider_config = config.provider_config();
    let api_key = provider_config
        .resolve_key()
        .ok_or_else(|| "没填 key：key 只能由使用者自己提供，这个页面不内置任何 key".to_string())?;
    // 恢复时能力只认 journal：当前宿主传入的能力不参与，不会把历史会话的声明
    // 覆盖掉。全新会话则使用本次 `AgentHost` 构造期解析出的能力，随后持久化。
    let (host_tools, host_skills, host_prefix) =
        capabilities_for_session(config, &session, restored);
    let mut ctx = RunnerCtx::new(
        config.adapter()?,
        Arc::new(Client::new()),
        provider_config.endpoint(),
        api_key,
        // 112 的注入接缝：浏览器没有文件系统，本地工具一件都执行不了。表里
        // 本来也没有声明任何本地工具，所以这个 executor 永远不会被查到。
        NullToolExecutor,
        // 122：三条内建、直接 host tool、skill 的 read/index，及开局块（157）。
        // 恢复走上面的 journal 快照；新会话才走构造 `AgentHost` 时给定的页面能力。
        crate::tools::browser_tool_table(&host_tools, host_skills, &host_prefix),
        vec![SystemChunk {
            label: Arc::from("base"),
            text: Arc::from(BASE_SYSTEM),
        }],
        SessionConfig {
            model: Arc::from(config.model.as_str()),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        store,
        // `new` 收的这条是不带归属的旧形状，本宿主不用它——真正的出口是下面
        // 的 `with_agent_events`。两条是**同一个字段**，后设的替换先设的。
        Box::new(|_| {}),
    )
    .with_agent_events(on_event)
    .with_remote_tool_timeout(HOST_TOOL_TIMEOUT);

    // 恢复之后必调：`persisted_seq` 这个同步水位不对齐，`persist::sync` 会把
    // `Session::restore` 灌回来的旧条目当新条目重新 append 一遍（CLI 那边抓到过
    // 的真 bug，见 `persist::seed_after_recover` 文档）。对全新会话是无害空操作。
    agent_runtime::persist::seed_after_recover(&mut ctx, &session);
    if !restored {
        agent_runtime::run_session_start(&mut session, ctx.tools()).map_err(|error| {
            format!(
                "会话开局能力初始化失败（{}）：{}",
                error.tool, error.message
            )
        })?;
        record_capabilities(&mut ctx, &mut session, config);
    }
    if needs_fail_close {
        // `Err` 与 `agent-cli::main` 同一条：报一次就够，装配继续——恢复期的
        // fail-close 本来就是「把旧轮收成终态」，它自己失败不该让会话开不起来。
        // 这个宿主没有 stderr，走 store 错误那条出口（页面已经在看它）。
        if let Err(failure) =
            agent_runtime::cancel_pending_remote_tools_async(&mut session, &mut ctx).await
        {
            report_store_error(format!("恢复期 fail-close 未能收尾：{failure:?}"));
        }
    }

    Ok(Live { id, session, ctx })
}

/// 新会话只写一次宿主能力；恢复路径已从 journal 取料，绝不覆盖或重复追加。
fn record_capabilities(ctx: &mut RunnerCtx, session: &mut Session, config: &HostConfig) {
    if !config.has_declared_capabilities() {
        return;
    }
    if !config.declared_tools().is_empty() {
        session.declare_host_tools(config.declared_tools().to_vec());
    }
    if !config.declared_skills().is_empty() {
        session.declare_host_skills(config.declared_skills().to_vec());
    }
    if !config.declared_prefix().is_empty() {
        session.declare_host_prefix(config.declared_prefix().to_vec());
    }
    session.begin_turn();
    agent_runtime::persist::sync(ctx, session);
}

/// 恢复会话的声明只从 journal 重放；构造当前 `AgentHost` 时传入的配置只可用于新会话。
fn capabilities_for_session(
    config: &HostConfig,
    session: &Session,
    restored: bool,
) -> (
    Vec<(agent_core::ToolSpec, agent_core::Reversibility)>,
    Vec<agent_core::HostSkill>,
    Vec<(Arc<str>, Arc<str>)>,
) {
    if restored {
        (
            session.host_tools(),
            session.host_skills(),
            session.host_prefix(),
        )
    } else {
        (
            config.declared_tools().to_vec(),
            config.declared_skills().to_vec(),
            config.declared_prefix().to_vec(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{HostSkill, Reversibility, SkillId, ToolSpec};

    fn tool(name: &str) -> (ToolSpec, Reversibility) {
        (
            ToolSpec {
                name: Arc::from(name),
                description: Arc::from("测试工具"),
                schema: Arc::new(serde_json::json!({"type":"object"})),
            },
            Reversibility::Pure,
        )
    }

    fn skill(id: &str) -> HostSkill {
        HostSkill {
            id: SkillId::new(id),
            description: Arc::from("测试 skill"),
            body: Arc::from("journal 正文"),
            tools: Vec::new(),
            tool_reversibility: Default::default(),
        }
    }

    fn config() -> HostConfig {
        HostConfig::parse(
            r#"{"provider":"deepseek","base_url":"https://example.invalid","model":"test","api_key":"test"}"#,
        )
        .expect("测试配置应有效")
        .with_declared_capabilities(
            vec![tool("web:current/tool")],
            vec![skill("current")],
            vec![(Arc::from("web:current/briefing"), Arc::from("当前配置的块"))],
        )
    }

    #[test]
    fn recovery_uses_journal_capabilities_instead_of_current_host_configuration() {
        let mut session = Session::new(AgentId::root());
        session.declare_host_tools(vec![tool("web:journal/tool")]);
        session.declare_host_skills(vec![skill("journal")]);
        session.declare_host_prefix(vec![(Arc::from("web:journal/briefing"), Arc::from("journal 块"))]);

        let (tools, skills, prefix) = capabilities_for_session(&config(), &session, true);

        assert_eq!(&*tools[0].0.name, "web:journal/tool");
        assert_eq!(skills[0].id.as_str(), "journal");
        assert_eq!(&*prefix[0].0, "web:journal/briefing");
        let table = crate::tools::browser_tool_table(&tools, skills, &prefix);
        assert!(table.declares("web:journal/tool"));
        assert!(table.declares("srv:skill/read"));
        assert!(!table.declares("web:current/tool"));
        // 合成的开局块条目住 timed 区，不进模型面（155 的既有语义）。
        assert!(!table.declares("web:journal/briefing"));
    }

    #[test]
    fn human_host_tool_wait_budget_is_ten_minutes() {
        assert_eq!(HOST_TOOL_TIMEOUT, std::time::Duration::from_secs(600));
    }
}
