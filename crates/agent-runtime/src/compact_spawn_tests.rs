//! `compact_spawn::intercept` 的单元面：任务文本怎么拼、模型怎么选、拒绝路径。
//!
//! 端到端「摘要子 agent 真的跑完、结果真的回到父」在
//! `compact_slot` 的测试里（那边不需要 `RunnerCtx`）；这里只测这个文件自己的
//! 职责——`Effect::Compact` 落地时该造出什么。

use agent_core::{AgentId, ExecutionProfileId, PrefixImage, StopReason, TokenUsage};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;
use serde_json::json;

use super::*;
use crate::tool_table::ToolTable;

/// 端点/密钥是占位（照 `collect_tool_tests.rs::build_ctx` 同款装配）。
fn build_ctx() -> RunnerCtx {
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "fake-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        agent_core::SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(|_| {}),
    )
}

/// 造一段有四条消息的历史：第 0/1 条要在摘要材料里出现，第 2/3 条在 `upto`
/// 之外，绝不能出现。
fn history_of_four(session: &mut Session, root: &AgentId) {
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("IN_BOUNDS_USER_TEXT"),
    });
    session.step(Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("IN_BOUNDS_ASSISTANT_TEXT"))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("OUT_OF_BOUNDS_USER_TEXT"),
    });
    session.step(Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("OUT_OF_BOUNDS_ASSISTANT_TEXT"))],
        stop: StopReason::EndTurn,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
}

/// 契约 1 + 「子怎么读到父的历史」：任务文本 = 固定指令 + `[0, upto)` 那段的
/// 纯文本渲染，`upto` 之外的一个字都不该出现。
#[test]
fn the_task_text_carries_the_fixed_instructions_and_only_the_in_bounds_history() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    history_of_four(&mut session, &root);
    let mut ctx = build_ctx();
    let mut compactions = CompactSlots::default();
    let epoch = session.epoch();

    let dispatched = intercept(&mut session, &mut ctx, &mut compactions, root.clone(), 2, epoch);

    let Dispatched::Event(Event::UserInput { agent: child, text }) = dispatched else {
        panic!("该产出子 agent 的第一条 user 消息");
    };
    assert_eq!(child, AgentId::new("root/a1"));
    assert!(text.starts_with(SUMMARY_INSTRUCTIONS), "指令段必须原样在场：{text}");
    assert!(text.contains("IN_BOUNDS_USER_TEXT"), "{text}");
    assert!(text.contains("IN_BOUNDS_ASSISTANT_TEXT"), "{text}");
    assert!(
        !text.contains("OUT_OF_BOUNDS"),
        "upto 之外的内容不该泄进摘要材料：{text}"
    );
}

/// 契约 2：子 agent 的模型来自 `ctx.compaction_execution_profile`，不是硬编码
/// 的默认值——设了就落进它的 `ChildConfig::execution_profile`。
#[test]
fn the_child_gets_the_hosts_configured_compaction_execution_profile() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("hi"),
    });
    let mut ctx = build_ctx();
    ctx.compaction_execution_profile = Some(ExecutionProfileId::new("cheap-summarizer"));
    let mut compactions = CompactSlots::default();
    let epoch = session.epoch();

    let dispatched = intercept(&mut session, &mut ctx, &mut compactions, root.clone(), 1, epoch);
    let Dispatched::Event(Event::UserInput { agent: child, .. }) = dispatched else {
        panic!("该产出子 agent 的第一条 user 消息");
    };

    assert_eq!(
        session.execution_profile_of(&child),
        Some(ExecutionProfileId::new("cheap-summarizer"))
    );
}

/// 没配 `compaction_execution_profile`（宿主没给）：子 agent 落 `None`，走
/// `RunnerCtx` 的默认 binding——不是「没配就摘要不了」。
#[test]
fn an_unconfigured_profile_leaves_the_child_on_the_default_binding() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("hi"),
    });
    let mut ctx = build_ctx();
    let mut compactions = CompactSlots::default();
    let epoch = session.epoch();

    let dispatched = intercept(&mut session, &mut ctx, &mut compactions, root.clone(), 1, epoch);
    let Dispatched::Event(Event::UserInput { agent: child, .. }) = dispatched else {
        panic!("该产出子 agent 的第一条 user 消息");
    };

    assert_eq!(session.execution_profile_of(&child), None);
}

/// 契约 3 + 4：spawn 被深度闸拒绝 → 当场 `CompactFailed`，`epoch` 原样带回，
/// 没有任何子 agent 被造出来、也没有任何一格记进 `CompactSlots`。
#[test]
fn a_refused_spawn_yields_compact_failed_with_the_same_epoch_and_records_nothing() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    session.set_agent_limits(agent_core::AgentLimits {
        max_depth: 0,
        max_children: 8,
        ..agent_core::AgentLimits::default()
    });
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("hi"),
    });
    let mut ctx = build_ctx();
    let mut compactions = CompactSlots::default();
    let epoch = session.epoch();

    let dispatched = intercept(&mut session, &mut ctx, &mut compactions, root.clone(), 1, epoch);

    let Dispatched::Event(event) = dispatched else {
        panic!("该产出一个事件");
    };
    assert_eq!(event, Event::CompactFailed { agent: root, epoch });
    assert!(
        compactions.harvest(&mut session).is_empty(),
        "没有子 agent 被造出来，`CompactSlots` 不该记下任何一格"
    );
}

/// `ToolUse`/`ToolResult` 保留在摘要材料里，`Thinking` 被过滤——跟
/// `child_outcome::final_text` 同一条判断（模块文档已经解释了为什么）。
#[test]
fn tool_activity_survives_rendering_but_thinking_does_not() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("go"),
    });
    session.step(Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![
            ContentBlock::Thinking(Arc::from("SECRET_CHAIN_OF_THOUGHT")),
            ContentBlock::ToolUse {
                id: agent_core::ToolCallId::new("call_1"),
                name: Arc::from("srv:fs/read"),
                input: Arc::new(json!({"path": "a.txt"})),
            },
        ],
        stop: StopReason::ToolUse,
        usage: TokenUsage {
            prompt: 1,
            completion: 1,
            cached: None,
        },
        prefix: PrefixImage {
            segments: Vec::new(),
            prompt_tokens: None,
        },
        adjustments: Vec::new(),
    });
    session.step(Event::ToolResult {
        agent: root.clone(),
        epoch: session.epoch(),
        call_id: agent_core::ToolCallId::new("call_1"),
        content: Arc::from("VISIBLE_TOOL_OUTPUT"),
    });
    let mut ctx = build_ctx();
    let mut compactions = CompactSlots::default();
    let epoch = session.epoch();
    let upto = session.messages_of(&root).len();

    let dispatched = intercept(&mut session, &mut ctx, &mut compactions, root.clone(), upto, epoch);
    let Dispatched::Event(Event::UserInput { text, .. }) = dispatched else {
        panic!("该产出子 agent 的第一条 user 消息");
    };

    assert!(!text.contains("SECRET_CHAIN_OF_THOUGHT"), "{text}");
    assert!(text.contains("srv:fs/read"), "{text}");
    assert!(text.contains("VISIBLE_TOOL_OUTPUT"), "{text}");
}
