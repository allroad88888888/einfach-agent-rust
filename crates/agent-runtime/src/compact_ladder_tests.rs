//! `Ladder` 的单元面：**判读时机**（一轮一次、必须有实测、只在 `Done` 上）与两档
//! 各自的执行形态。判读本身（哪一档、清哪些、摘到哪）是 `agent-core` 那个纯函数
//! 的事，它自己有 inline 单测，这里不重复。

use std::sync::Arc;

use agent_core::{ContentBlock, PrefixImage, SessionConfig, StopReason, TokenUsage, ToolCallId};
use agent_providers::deepseek::DeepSeek;
use agent_tools::ToolExecutor;
use agent_transport::Client;

use super::*;
use crate::tool_table::ToolTable;

/// 端点/密钥是占位（照 `compact_spawn_tests::build_ctx` 同款装配），只有
/// `context_window` 是这些测试真正要拨的旋钮。
fn build_ctx(context_window: Option<u32>) -> RunnerCtx {
    RunnerCtx::new(
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
        Box::new(|_| {}),
    )
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

/// 一条终答，把 root 推到 `Done`，同时把 `prompt` 回填进 `PrevPrefix`
/// （`provider_done` 那一格干的，见 `compact_ladder` 模块文档）。
fn answer(session: &mut Session, root: &AgentId, prompt: u32) -> Event {
    let event = Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from("答完了"))],
        stop: StopReason::EndTurn,
        usage: usage(prompt),
        prefix: prefix(),
        adjustments: Vec::new(),
    };
    session.step(event.clone());
    event
}

/// 跑一轮：用户问一句 → 一次工具往返 → 终答。留下 4 条消息
/// （`User` / `Assistant(ToolUse)` / `Assistant(ToolResult)` / `Assistant(Text)`）。
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
    answer(session, root, prompt);
}

/// 五轮工具会话 + 一个 `Ladder`，实测用量 `prompt`（窗口固定 1000）。
fn five_tool_turns(prompt: u32) -> (Session, RunnerCtx, Ladder, AgentId) {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    let mut ladder = Ladder::new(root.clone());
    for n in 1..=5 {
        tool_turn(&mut session, &root, n, prompt);
    }
    // 这五轮里最后那条终答就是「这一轮的实测」。
    ladder.note(&Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: Vec::new(),
        stop: StopReason::EndTurn,
        usage: usage(prompt),
        prefix: prefix(),
        adjustments: Vec::new(),
    });
    (session, build_ctx(Some(1000)), ladder, root)
}

/// 压力超线且有够得着的工具结果 → 第 2 档：**不产出任何 effect**（清除是一条
/// 命令，当场写完），`SendPlan` 上多出保护区之外那两条。
#[test]
fn tier_two_writes_the_plan_and_produces_no_effect() {
    let (mut session, mut ctx, mut ladder, root) = five_tool_turns(900);
    let before = session.history_len();

    let effects = ladder.fire_once(&mut session, &mut ctx);

    assert!(effects.is_empty(), "第 2 档不该产出 effect");
    assert_eq!(
        session.send_plan_of(&root).cleared(),
        [ToolCallId::new("call_1"), ToolCallId::new("call_2")].as_slice(),
        "最近 3 轮之外的工具结果一次全清，最老优先"
    );
    assert_eq!(session.history_len(), before + 1, "清除是一条 journaled entry");
}

/// 第 2 档已经清光（同一份历史、同样的压力）→ 第 3 档：恰好一条
/// `Effect::Compact`，`upto` 是保护区起点，`epoch` 是当前世代。
#[test]
fn tier_three_emits_exactly_one_compact_effect() {
    let (mut session, mut ctx, mut ladder, root) = five_tool_turns(900);
    session.clear_tool_results(&root, [ToolCallId::new("call_1"), ToolCallId::new("call_2")]);

    let effects = ladder.fire_once(&mut session, &mut ctx);

    assert_eq!(
        effects,
        vec![Effect::Compact {
            agent: root,
            // 5 轮 × 4 条消息，最近 3 轮从第 3 轮的 `User`（下标 8）起。
            upto: 8,
            epoch: session.epoch(),
        }]
    );
}

/// **一轮只判一次**：第 2 档开火之后再问，什么都不做。没有这个闩的话，泵下一次
/// 静止时第 2 档已经返回空、第 3 档会在同一轮里接上——「清完还不够」就从下一轮
/// 实测退化成同一轮的推断（108 §「为什么阶梯是跨轮的」）。
#[test]
fn the_ladder_fires_at_most_once_per_turn() {
    let (mut session, mut ctx, mut ladder, root) = five_tool_turns(900);

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    let after_tier_two = session.history_len();

    for _ in 0..5 {
        assert!(
            ladder.fire_once(&mut session, &mut ctx).is_empty(),
            "同一轮里第 3 档绝不该接上来"
        );
    }
    assert_eq!(session.history_len(), after_tier_two, "也没有第二次清除");
    assert_eq!(session.send_plan_of(&root).boundary(), 0, "边界一格没动");
}

/// 压力没超：一档都不开火，状态一个字节不动（反向锁）。
#[test]
fn below_the_trigger_line_nothing_happens() {
    let (mut session, mut ctx, mut ladder, root) = five_tool_turns(500);
    let before = session.history_len();

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
    assert_eq!(session.history_len(), before);
}

/// `context_window: None`（未知/不设限）：两档都不触发，不 panic。
#[test]
fn an_unknown_context_window_fires_nothing() {
    let (mut session, _ctx, mut ladder, root) = five_tool_turns(900);
    let mut ctx = build_ctx(None);

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
}

/// 这一轮没有 root 的实测（`note` 一次都没喊到）：不判。拿上一轮的实测再判一次，
/// 会凭空多开一次第 3 档——而「清完还不够」从来没被实测过。
#[test]
fn without_a_fresh_measurement_the_ladder_stays_quiet() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    for n in 1..=5 {
        tool_turn(&mut session, &root, n, 900);
    }
    let mut ctx = build_ctx(Some(1000));
    let mut ladder = Ladder::new(root.clone());

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
}

/// `note` 只认 **root 自己**的 `ProviderDone`：子 agent 答完一句、或者别的事件，
/// 都不算这一轮拿到了实测。
#[test]
fn only_the_roots_own_provider_done_counts_as_a_measurement() {
    let (mut session, mut ctx, _ladder, root) = five_tool_turns(900);
    let child = AgentId::new("root/a1");
    let mut ladder = Ladder::new(root.clone());

    ladder.note(&Event::UserInput {
        agent: root.clone(),
        text: Arc::from("hi"),
    });
    ladder.note(&Event::ProviderDone {
        agent: child,
        epoch: session.epoch(),
        blocks: Vec::new(),
        stop: StopReason::EndTurn,
        usage: usage(900),
        prefix: prefix(),
        adjustments: Vec::new(),
    });

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
}

/// 取消收尾的一轮（`Failed(Cancelled)`，也是终态）不压：用户刚按下取消，紧接着
/// 起一个摘要子 agent 是最不该发生的事。
#[test]
fn a_cancelled_turn_never_compacts() {
    let (mut session, mut ctx, mut ladder, root) = five_tool_turns(900);
    session.begin_turn();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("再问一句"),
    });
    session.step(Event::Cancel { agent: root.clone() });
    assert!(session.status().is_terminal());

    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
}

/// root 还没落终态（工具还在飞）：不判——判读时机是「turn 结束拿到 usage 时」。
#[test]
fn a_turn_still_in_flight_is_not_judged() {
    let root = AgentId::root();
    let mut session = Session::new(root.clone());
    for n in 1..=5 {
        tool_turn(&mut session, &root, n, 900);
    }
    session.begin_turn();
    session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("再问一句"),
    });
    let mut ladder = Ladder::new(root.clone());
    ladder.note(&answer_event(&session, &root, 900));
    let mut ctx = build_ctx(Some(1000));

    assert!(!session.status().is_terminal());
    assert!(ladder.fire_once(&mut session, &mut ctx).is_empty());
    assert!(session.send_plan_of(&root).cleared().is_empty());
}

/// 造一条 root 的 `ProviderDone`（只喂给 `Ladder::note`，不推进状态）。
fn answer_event(session: &Session, root: &AgentId, prompt: u32) -> Event {
    Event::ProviderDone {
        agent: root.clone(),
        epoch: session.epoch(),
        blocks: Vec::new(),
        stop: StopReason::EndTurn,
        usage: usage(prompt),
        prefix: prefix(),
        adjustments: Vec::new(),
    }
}
