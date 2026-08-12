//! `CompactSlots` 的单元面：收割翻译、`upto` 的转存、一次性工人的回收。
//!
//! 不需要 `RunnerCtx`——收割这一步只碰 `Session`（despawn 也是一条纯 command），
//! 所以这些测试全部零装配、零 IO。

use agent_core::{ChildConfig, ContentBlock, PrefixImage, StopReason, TokenUsage};

use super::*;

fn spawn_child(session: &mut Session, parent: &AgentId) -> AgentId {
    session
        .spawn_child(parent, ChildConfig::default(), None)
        .expect("root 是活的，深度/子数都在默认上限内")
}

/// 让 `child` 答完一句话并落 `Done`。
fn finish_child(session: &mut Session, child: &AgentId, text: &str) {
    session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("摘要一下"),
    });
    session.step(Event::ProviderDone {
        agent: child.clone(),
        epoch: session.epoch(),
        blocks: vec![ContentBlock::Text(Arc::from(text))],
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

/// 子 agent 还没到终态：留在表里，不产出任何事件，也不回收。
#[test]
fn a_non_terminal_child_yields_nothing_and_stays_recorded() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = spawn_child(&mut session, &root);

    let mut slots = CompactSlots::default();
    slots.record(child.clone(), root.clone(), session.epoch(), 3);

    assert!(slots.harvest(&mut session).is_empty());
    assert_eq!(slots.slots.len(), 1, "未终止的槽位原样留着");
    assert!(session.is_live(&child), "还在干活的子不该被回收");
}

/// 子 agent 正常答完：`CompactDone`，正文等于它的终答，`epoch` 原样带回；
/// 回写意图（带 `upto`）记进 `awaiting_gate`；**子 agent 当场被回收**。
#[test]
fn a_done_child_yields_compact_done_records_the_writeback_and_is_reaped() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = spawn_child(&mut session, &root);
    let epoch = session.epoch();
    finish_child(&mut session, &child, "这是摘要正文");

    let mut slots = CompactSlots::default();
    slots.record(child.clone(), root.clone(), epoch, 7);

    let events = slots.harvest(&mut session);
    assert_eq!(
        events,
        vec![Event::CompactDone {
            agent: root.clone(),
            summary: Arc::from("这是摘要正文"),
            epoch,
        }]
    );
    assert!(slots.slots.is_empty(), "收割之后这一格该被划掉");
    assert!(!session.is_live(&child), "一次性工人收割完就该 despawn");

    let pending = slots
        .take_gated_summary(&root, epoch)
        .expect("回写意图该在等着过闸");
    assert_eq!(pending.upto, 7, "`upto` 只有这一侧记着");
    assert_eq!(pending.summary.as_ref(), "这是摘要正文");
    assert!(
        slots.take_gated_summary(&root, epoch).is_none(),
        "取走即消费，一份摘要只回写一次"
    );
}

/// 连续 10 次压缩：每次收割都回收，`max_children` 默认 8 一次都没撞到。
/// 这条是 108 那个裁决的度量——不回收的话第 9 次必红。
#[test]
fn ten_consecutive_compactions_never_run_out_of_child_slots() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    assert_eq!(session.agent_limits().max_children, 8, "默认上限没变");

    for round in 0..10 {
        let child = session
            .spawn_child(&root, ChildConfig::default(), None)
            .unwrap_or_else(|e| panic!("第 {} 次压缩 spawn 就被拒了：{e:?}", round + 1));
        let epoch = session.epoch();
        finish_child(&mut session, &child, "摘要");

        let mut slots = CompactSlots::default();
        slots.record(child, root.clone(), epoch, round + 1);
        assert_eq!(slots.harvest(&mut session).len(), 1);
    }
}

/// 子 agent 被取消：算失败，`CompactFailed`（不是 `CompactDone`），
/// **不记回写意图**，但同样回收。
#[test]
fn a_cancelled_child_yields_compact_failed_and_records_no_writeback() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = spawn_child(&mut session, &root);
    let epoch = session.epoch();

    session.step(Event::UserInput {
        agent: child.clone(),
        text: Arc::from("摘要一下"),
    });
    session.step(Event::Cancel {
        agent: child.clone(),
    });

    let mut slots = CompactSlots::default();
    slots.record(child.clone(), root.clone(), epoch, 3);

    assert_eq!(
        slots.harvest(&mut session),
        vec![Event::CompactFailed {
            agent: root.clone(),
            epoch
        }]
    );
    assert!(slots.awaiting_gate.is_empty(), "失败不该留下回写意图");
    assert!(!session.is_live(&child));
}

/// 世代推走之后那份回写意图必须消失——留着就是给一份属于旧世界的摘要留一条
/// 将来可能被写进去的路（红线 6）。
#[test]
fn a_stale_writeback_is_dropped_and_can_never_be_taken() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = spawn_child(&mut session, &root);
    let epoch = session.epoch();
    finish_child(&mut session, &child, "摘要");

    let mut slots = CompactSlots::default();
    slots.record(child, root.clone(), epoch, 4);
    let _ = slots.harvest(&mut session);

    // 用户按了取消：世代推走一格。
    session.step(Event::Cancel {
        agent: root.clone(),
    });
    let now = session.epoch();
    assert_ne!(now, epoch);

    slots.drop_stale_summaries(now);
    assert!(slots.take_gated_summary(&root, epoch).is_none());
    assert!(slots.take_gated_summary(&root, now).is_none());
}
