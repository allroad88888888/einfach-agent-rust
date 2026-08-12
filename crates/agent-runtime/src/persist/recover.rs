//! 崩溃恢复：`SessionStore::load()` 有货 → 翻译 meta → `Session::restore`。
//!
//! 「恢复就是 redo」（010）的最后一段管道在这里接通：011 的端口给出一个三态
//! `LoadOutcome<AtomKey, AgentValue, PersistedMeta>`（`agent_store::persist::
//! LoadOutcome`，见其文档「契约更正」一节——原来是 `Option`，独测发现「中部损坏」
//! 和「从没写过」被压缩成同一个 `None` 是真 bug），这个文件把 `Loaded` 那一态里的
//! `PersistedMeta` 翻回 `agent_core::EntryMeta`（[`super::meta`] 那张对照表），再调
//! [`agent_core::Session::restore`]（026/027 新增）真正重建图。
//!
//! **在飞工具槽不自动重发**：`has_unresolved_tool_calls` 是宿主判断「这个恢复出来的
//! 会话里有没有一个工具调用发出去了、结果还没落地」的唯一入口——020 推迟的账，
//! `Interrupted { may_have_executed }` 的运行时语义在这里兑现：不是一个新的状态变体，
//! 是宿主看到 `ToolsPending` 且未收敛就知道「上一个进程可能已经把这个工具跑了，
//! 不能揣着 `ToolCallRequest` 假装它没发生过就重新 `ExecuteTool`」。

use agent_core::{AgentId, AgentLimits, Session, TurnStatus};
use agent_store::history::Entry as PersistedEntry;
use agent_store::persist::LoadOutcome;

use super::SessionBackend;
use super::meta::UnknownLabel;

/// 恢复失败的三种理由，**出口统一**：main.rs 只有一处 `Err(e) => fail(...)`，
/// 三者都走它，都以非零退出码硬失败，错误文本都不带 K/V 内容。
///
/// - `Refused`：`SessionStore::load()` 自己拒绝——`LoadOutcome::Refused` 翻译过来，
///   中部损坏、结构不满足实现自身的完整性要求（`Jsonl::load()` 的 `CorruptLine`
///   分支）。**这是文件损坏**，跟下面两条不一样：`reason` 来自
///   `agent_runtime::SessionStoreError` 的 `Display`，同样不带内容。
/// - `UnknownLabel`：语法合法，但标签字符串是这一版代码没见过的历史值。
/// - `InvalidHistory`：三元组本身不满足 `History::from_parts` 的不变量（比如
///   手工改过的文件、版本回退）。
///
/// 后两者发生在 `LoadOutcome::Loaded` 之后——`Jsonl`/`Memory` 自己认为这份数据
/// 完整，但翻译/重建这一层发现语义不对，同样不吞、不猜、不给一个能跑但是错的会话。
#[derive(Debug)]
pub enum RecoverError {
    Refused(String),
    UnknownLabel(UnknownLabel),
    InvalidHistory(agent_store::InvalidHistory),
}

impl std::fmt::Display for RecoverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RecoverError::Refused(reason) => write!(f, "{reason}"),
            RecoverError::UnknownLabel(e) => write!(f, "{e}"),
            RecoverError::InvalidHistory(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for RecoverError {}

/// `store.load()` 为 `Absent`（全新会话，从没写过东西）→ `Ok(None)`；`Refused`
/// （有会话但读不出来）→ `Err`，硬失败，不当成「没有会话」；`Loaded` 就翻译 +
/// 重建，翻译/重建失败同样原样报告，**不吞、不猜、不给一个能跑但是错的会话**。
///
/// `history_cap` 与 `limits` 是**这个会话的两项配置**：都不进原子图、不进日志，
/// 所以都恢复不出来，都必须由宿主把自己那一份再说一遍（`Session::restore` 的文档
/// 「`limits` 为什么必须是入参」）。两个参数排在一起就是为了让这层意思读得出来
/// ——160 之前只有 `history_cap` 有通道，`limits` 在 `restore` 里被硬写成默认值，
/// 上限一可配就是一处静默失配。
pub fn recover(
    store: &SessionBackend,
    agent: AgentId,
    history_cap: usize,
    limits: AgentLimits,
    on_unknown_key: &mut impl FnMut(&agent_core::AtomKey),
) -> Result<Option<Session>, RecoverError> {
    let loaded = match store.load() {
        LoadOutcome::Absent => return Ok(None),
        LoadOutcome::Refused { reason } => return Err(RecoverError::Refused(reason)),
        LoadOutcome::Loaded(loaded) => loaded,
    };

    let mut entries = Vec::with_capacity(loaded.entries.len());
    for e in loaded.entries {
        let meta = agent_core::EntryMeta::try_from(e.meta).map_err(RecoverError::UnknownLabel)?;
        entries.push(PersistedEntry {
            seq: e.seq,
            meta,
            changes: e.changes,
        });
    }
    let snapshot = loaded.snapshot.map(|s| s.values);

    let session = Session::restore(
        agent,
        snapshot,
        entries,
        loaded.cursor,
        loaded.next_seq,
        history_cap,
        limits,
        on_unknown_key,
    )
    .map_err(RecoverError::InvalidHistory)?;
    Ok(Some(session))
}

/// 恢复出来的会话里有没有一个工具调用「发出去了、还没收到结果」——宿主据此决定
/// 「打一句可能已经执行过，不重发」，而不是照单把 `ToolCallRequest` 重新
/// `Effect::ExecuteTool` 一遍。
pub fn has_unresolved_tool_calls(session: &Session) -> bool {
    matches!(session.status(), TurnStatus::ToolsPending) && !session.tools_converged()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent_core::{ContentBlock, Event, SlotState, ToolCallId};
    use agent_store::persist::Memory;

    use super::*;
    use crate::ctx::RunnerCtx;
    use crate::persist::meta::PersistedMeta;
    use crate::persist::sync::sync;
    use crate::tool_table::ToolTable;
    use agent_providers::deepseek::DeepSeek;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;

    type Backend = Memory<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>;

    /// `Memory` 直接塞进 `Box<dyn SessionStore<..>>`——测试结束前 `ctx` 一直持有它，
    /// `recover` 只需要 `&dyn SessionStore`，从 `ctx.session_store` 借出来读就够了，
    /// 不需要额外的共享句柄或转发包装。
    fn ctx() -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        let store: Box<SessionBackend> = Box::new(Backend::new());
        RunnerCtx::new(
            Arc::new(DeepSeek),
            Arc::new(Client::new()),
            "https://api.deepseek.com/chat/completions".to_string(),
            "key".to_string(),
            fs,
            ToolTable::builtin(),
            Vec::new(),
            agent_core::SessionConfig {
                model: Arc::from("m"),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
            store,
            Box::new(|_| {}),
        )
    }

    /// 从没写过东西：`recover` 返回 `Ok(None)`，不是错误。
    #[test]
    fn a_backend_that_was_never_written_to_recovers_to_none() {
        let ctx = ctx();
        let got = recover(
            ctx.session_store.as_ref(),
            AgentId::root(),
            100,
            agent_core::AgentLimits::default(),
            &mut |_| {},
        )
        .unwrap();
        assert!(got.is_none());
    }

    /// bug 2 的回归测试（快速、不经 CLI 子进程版）：`SessionStore::load()` 给出
    /// `LoadOutcome::Refused` → `recover()` 必须是 `Err(RecoverError::Refused(..))`，
    /// **不是** `Ok(None)`——三态化之前的真 bug 正是把这两者压成了同一个值，
    /// `main.rs` 拿到 `Ok(None)` 就会当成「没有会话」开新的，下一张快照覆盖用户
    /// 原文件。这里用一个恒返回 `Refused` 的假 `SessionStore` 直接钉住 `recover`
    /// 自己的翻译逻辑，端到端版本（真 `Jsonl` + 真 CLI 子进程 + 断言原文件字节
    /// 不变）在 `agent-cli/tests/indep_corrupt_session.rs`。
    struct AlwaysRefuses;
    impl agent_store::SessionStore<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>
        for AlwaysRefuses
    {
        fn append(
            &self,
            _: &agent_store::Entry<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta>,
        ) {
        }
        fn drop_oldest(&self, _: usize) {}
        fn drop_after(&self, _: u64, _: usize) {}
        fn set_cursor(&self, _: usize) {}
        fn snapshot(&self, _: &agent_store::Snapshot<agent_core::AtomKey, agent_core::AgentValue>) {
        }
        fn load(&self) -> LoadOutcome<agent_core::AtomKey, agent_core::AgentValue, PersistedMeta> {
            LoadOutcome::Refused {
                reason: "会话文件第 1 行损坏（非法记录），拒绝加载".to_string(),
            }
        }
    }

    #[test]
    fn a_refused_load_is_a_hard_error_not_an_empty_session() {
        let store = AlwaysRefuses;
        let result = recover(
            &store,
            AgentId::root(),
            100,
            agent_core::AgentLimits::default(),
            &mut |_| panic!("不该有不认识的键"),
        );
        match result {
            Err(RecoverError::Refused(reason)) => {
                assert!(reason.contains('1'), "理由该带行号：{reason}")
            }
            Err(other) => panic!("该是 RecoverError::Refused，实际：{other:?}"),
            Ok(_) => panic!("Refused 必须变成 Err，不能被当成 Ok(None)（没有会话）悄悄放过"),
        }
    }

    /// 写几轮、sync 到 store → `recover` 重建出的会话状态和原会话一致，undo 栈还能用。
    #[test]
    fn a_session_written_through_sync_recovers_with_matching_state_and_a_working_undo_stack() {
        let mut ctx = ctx();
        let mut session = Session::new(AgentId::root());

        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("hi"),
        });
        sync(&mut ctx, &mut session);
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![ContentBlock::Text(Arc::from("ok"))],
            stop: agent_core::StopReason::EndTurn,
            usage: agent_core::TokenUsage {
                prompt: 1,
                completion: 1,
                cached: None,
            },
            prefix: agent_core::PrefixImage {
                segments: Vec::new(),
                prompt_tokens: Some(1),
            },
            adjustments: Vec::new(),
        });
        sync(&mut ctx, &mut session);

        let mut recovered = recover(
            ctx.session_store.as_ref(),
            AgentId::root(),
            100,
            agent_core::AgentLimits::default(),
            &mut |_| panic!("不该有不认识的键"),
        )
        .unwrap()
        .expect("写过东西该恢复出 Some");

        assert_eq!(recovered.status(), session.status());
        assert_eq!(recovered.messages().len(), session.messages().len());

        // undo 栈还能用：撤一次应该退回 Thinking。
        let report = recovered.undo_turn();
        assert!(matches!(report, agent_core::UndoReport::Applied { .. }));
    }

    /// 未收敛的工具槽：`ToolsPending` 且至少一个 `Pending` → `has_unresolved_tool_calls`
    /// 为真，宿主据此不重发。
    #[test]
    fn unresolved_tool_calls_are_detected() {
        let mut session = Session::new(AgentId::root());
        let _ = session.step(Event::UserInput {
            agent: AgentId::root(),
            text: Arc::from("hi"),
        });
        let _ = session.step(Event::ProviderDone {
            agent: AgentId::root(),
            epoch: session.epoch(),
            blocks: vec![agent_core::ContentBlock::ToolUse {
                id: ToolCallId::new("call_1"),
                name: Arc::from("srv:shell/exec"),
                input: Arc::new(serde_json::json!({"cmd": "echo hi"})),
            }],
            stop: agent_core::StopReason::ToolUse,
            usage: agent_core::TokenUsage {
                prompt: 1,
                completion: 1,
                cached: None,
            },
            prefix: agent_core::PrefixImage {
                segments: Vec::new(),
                prompt_tokens: Some(1),
            },
            adjustments: Vec::new(),
        });

        assert!(has_unresolved_tool_calls(&session));
        assert!(matches!(session.tool_slots()[0].state, SlotState::Pending));
    }
}
