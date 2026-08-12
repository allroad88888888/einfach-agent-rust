//! 109：`RunnerEvent::CompactionApplied`/`ToolResultsCleared` 真的从各自的
//! 触发点发出去，字段跟状态里落的那份对得上。
//!
//! 不是复用别的模块已有的测试文件：`compact_slot_tests.rs`/
//! `compact_ladder_tests.rs` 都只测「状态改对了没」，不碰事件出口；
//! `agent-server` 那边 `event/from_runner.rs` 的测试测的是「翻译对不对」
//! （`RunnerEvent` → `SessionEvent`），不是「真的从触发点发出来」。这两条各测
//! 一个新变体的发射点。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use agent_core::{
    AgentId, ChildConfig, ContentBlock, Event, PrefixImage, Session, SessionConfig, StopReason,
    TokenUsage, ToolCallId,
};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::RunnerCtx;
use crate::compact_ladder::Ladder;
use crate::compact_slot::CompactSlots;
use crate::compact_writeback;
use crate::event::RunnerEvent;
use crate::tool_table::ToolTable;

/// 装配一个丢弃 IO 但捕获事件的 `RunnerCtx`——跟
/// `transient_source_completion_tests::test_ctx` 同一个模式（`Rc<RefCell<Vec<_>>>`
/// 是单线程测试专用的观察点，不是生产形状）。
fn test_ctx(context_window: Option<u32>) -> (RunnerCtx, Rc<RefCell<Vec<RunnerEvent>>>) {
    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let ctx = RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(Client::new()),
        "http://127.0.0.1:1/chat/completions".to_string(),
        "fake-key".to_string(),
        ToolExecutor::new(std::env::temp_dir()).unwrap(),
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window,
        },
        crate::persist::open_backend(None, |_| {}),
        Box::new(move |event| observed.borrow_mut().push(event)),
    );
    (ctx, events)
}

fn usage(prompt: u32) -> TokenUsage {
    TokenUsage {
        prompt,
        completion: 1,
        cached: None,
    }
}

fn prefix() -> PrefixImage {
    PrefixImage {
        segments: Vec::new(),
        prompt_tokens: None,
    }
}

/// 109：`Session::apply_summary` 成功之后，`compact_writeback::after_step`
/// 该发一条 `RunnerEvent::CompactionApplied`——`turn_id`/`upto` 跟这次回写用的
/// 值一致，`summary_id` 跟 `summary_library` 里落的那一条**是同一个**（不去猜
/// 它的编码格式，直接比对状态）。
#[test]
fn a_successful_summary_writeback_emits_compaction_applied_with_matching_fields() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("你好"),
    });

    let child = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("root 是活的，深度/子数都在默认上限内");
    let epoch = session.epoch();
    session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("摘要一下"),
    });
    session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("这是摘要正文"))],
        stop: StopReason::EndTurn,
        usage: usage(1),
        prefix: prefix(),
        adjustments: Vec::new(),
    });

    let mut slots = CompactSlots::default();
    slots.record(child, root.clone(), epoch, 5);
    let harvested = slots.harvest(&mut session);
    assert_eq!(harvested.len(), 1, "子 agent 该已经落终态");

    let mut effects = Vec::new();
    for event in harvested {
        effects = session.step(event);
    }
    assert!(
        compact_writeback::passed_epoch_gate(&effects),
        "epoch 没变过，这份回执必须过闸"
    );

    let (mut ctx, captured) = test_ctx(None);
    compact_writeback::after_step(&mut session, &mut ctx, &mut slots, &root, &effects);

    let library = session.summary_library(&root);
    let (summary_id, text) = library.last().expect("成功回写之后库里该有一条");
    assert_eq!(text.as_ref(), "这是摘要正文");

    let captured = captured.borrow();
    assert_eq!(captured.len(), 1, "该恰好发一条 CompactionApplied");
    match &captured[0] {
        RunnerEvent::CompactionApplied {
            turn_id,
            upto,
            summary_id: emitted_id,
        } => {
            assert_eq!(*turn_id, session.turn_id());
            assert_eq!(*upto, 5);
            assert_eq!(emitted_id, summary_id, "发出去的 id 该跟库里那条是同一个");
        }
        other => panic!("期待 CompactionApplied，收到 {other:?}"),
    }
}

/// 109：`Ladder::fire_once` 走第 2 档、真清了东西之后，该发一条
/// `RunnerEvent::ToolResultsCleared`——`call_ids` 跟 `SendPlan::cleared()` 一致
/// （最近 3 轮之外那两个调用，最老优先）。
#[test]
fn a_tier_two_clear_emits_tool_results_cleared_with_the_newly_cleared_ids() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let mut ladder = Ladder::new(root.clone());
    for n in 1..=5 {
        tool_turn(&mut session, &root, n, 900);
    }
    ladder.note(&Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: Vec::new(),
        stop: StopReason::EndTurn,
        usage: usage(900),
        prefix: prefix(),
        adjustments: Vec::new(),
    });

    let (mut ctx, captured) = test_ctx(Some(1000));
    let effects = ladder.fire_once(&mut session, &mut ctx);
    assert!(effects.is_empty(), "第 2 档不产出 effect");

    let expected = [ToolCallId::new("call_1"), ToolCallId::new("call_2")];
    assert_eq!(
        session.send_plan_of(&root).cleared(),
        expected.as_slice(),
        "先钉住状态本身没走样"
    );

    let captured = captured.borrow();
    assert_eq!(captured.len(), 1, "该恰好发一条 ToolResultsCleared");
    match &captured[0] {
        RunnerEvent::ToolResultsCleared { turn_id, call_ids } => {
            assert_eq!(*turn_id, session.turn_id());
            assert_eq!(call_ids.as_slice(), expected.as_slice());
        }
        other => panic!("期待 ToolResultsCleared，收到 {other:?}"),
    }
}

/// 跑一轮：用户问一句 → 一次工具往返 → 终答。留下 4 条消息，跟
/// `compact_ladder_tests::tool_turn` 同一个形状（那份是模块私有的，够不着，
/// 这里独立造一份而不是改它的可见性——两个测试文件各自独立，互不依赖）。
fn tool_turn(session: &mut Session, root: &AgentId, n: usize, prompt: u32) {
    if session.status().is_terminal() {
        session.begin_turn();
    }
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("问一句"),
    });
    let call = ToolCallId::new(format!("call_{n}"));
    session.step(Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::ToolUse {
            id: call.clone(),
            name: Arc::from("srv:fs/read"),
            input: Arc::new(serde_json::json!({})),
        }],
        stop: StopReason::ToolUse,
        usage: usage(prompt),
        prefix: prefix(),
        adjustments: Vec::new(),
    });
    session.step(Event::ToolResult {
        agent: root.clone(),
        epoch: session.epoch(),
        call_id: call,
        content: Arc::from("一大段工具输出"),
    });
    session.step(Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("答完了"))],
        stop: StopReason::EndTurn,
        usage: usage(prompt),
        prefix: prefix(),
        adjustments: Vec::new(),
    });
}
