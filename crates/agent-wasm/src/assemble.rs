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
//! 那一段（122），没有 `srv:`、没有 `mcp:`、不开 spawn/status/collect/skill。
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
const BASE_SYSTEM: &str =
    "你是一个简洁、诚实的助手，跑在用户的浏览器里。需要页面本身的信息（标题、地址）时调对应的工具，不要猜。";

/// 一条 `web:` 工具等页面回调的截止线（123）。**比 native 默认短一个数量级**
/// （`agent_runtime::ctx::DEFAULT_REMOTE_TOOL_TIMEOUT` = 10 分钟），因为那个数是
/// 按「另一头是个真人：读完问题、切个标签页、回来作答」定的，而这一头是同一个
/// 标签页里的一个 JS 回调——机器干活。
///
/// 60 秒的两个边：
///
/// - **下边界**由这个里程碑里最慢的一条合法回调定：一张浏览器侧上限 2 MB 的图
///   （119 §五-1）走 multipart 上传 + 一次识图往返，弱网上几十秒是可能的。60s ≈ 那个
///   量级的两倍，留了余量。
/// - **上边界**由「挂住的代价」定，而浏览器这边的代价比 server 大得多：`send()`
///   在整轮期间握着 `live.borrow_mut()`，所以一条挂住的回调不是「一次调用慢」，
///   是整个 `AgentHost` 对页面失去响应（[`crate::host_session`] 的借用纪律）。
///   server 形态下 actor 只是空闲着，代价小，所以它能忍 10 分钟。
///
/// 取消是更快的那条逃生舱（用户一按立刻生效，见 `AgentHost::cancel`），所以这条线
/// 只需要兜住**没人看着**的那种挂死。
///
/// 不做成页面可配：那是宿主声明自己能力时该一起带进来的东西
/// （HOST-CAPABILITIES.md §四），122 之前没有那个入口，现在给的是一个固定默认
/// 加 `RunnerCtx::with_remote_tool_timeout` 这个既有的口。
const HOST_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

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

    let mut session = match agent_runtime::recover(
        store.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        // 快照里有这一版不认识的键：忽略并继续，跟 CLI 同一条处理。
        &mut |_key| {},
    ) {
        Ok(Some(recovered)) => recovered,
        Ok(None) => Session::new(AgentId::root()),
        // `Refused` = journal 读不回来。**硬失败**，不静默当成新会话——下一张
        // 快照就会把现场覆盖掉（`LoadOutcome` 文档里那条真 bug）。
        Err(error) => return Err(format!("会话恢复失败：{error}")),
    };
    let needs_fail_close = agent_runtime::recovered_transient_source_needs_fail_close(&session);

    let provider_config = config.provider_config();
    let api_key = provider_config
        .resolve_key()
        .ok_or_else(|| "没填 key：key 只能由使用者自己提供，这个页面不内置任何 key".to_string())?;
    let mut ctx = RunnerCtx::new(
        config.adapter()?,
        Arc::new(Client::new()),
        provider_config.endpoint(),
        api_key,
        // 112 的注入接缝：浏览器没有文件系统，本地工具一件都执行不了。表里
        // 本来也没有声明任何本地工具，所以这个 executor 永远不会被查到。
        NullToolExecutor,
        // 122：三条内建 + 页面在建宿主时声明的那一段（料在 `config` 上，它建宿主
        // 那一刻就定死了——同一个 `AgentHost` 开多少次会话都是同一份字节）。
        crate::tools::browser_tool_table(config.declared_tools()),
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
