//! 212 验收 4：**恢复之后仍然查得了环**——建一条 `await` → 落盘 → 恢复 →
//! 反向 `await` 仍然被拒。等待图必须是 journaled 状态：放内存里，一次崩溃
//! 恢复就会把查环能力丢掉，而丢了不报错。
//!
//! 照既有 `session_subagent_restore.rs` 的写法：`primitives()` + `history()` +
//! `cursor()` 拼出快照与日志，喂给公开的回放入口 `Session::restore`。

use agent_core::value::awaiting::AwaitUntil;
use agent_core::{AgentEntry, AgentId, AwaitDenied, ChildConfig, Session};

fn restore_from(live: &Session) -> Session {
    let root = AgentId::root();
    let snapshot = live.primitives();
    let entries: Vec<AgentEntry> = live.history().entries().cloned().collect();
    let cursor = live.cursor();
    let next_seq = entries.last().map_or(0, |e| e.seq + 1);
    Session::restore(
        root,
        Some(snapshot),
        entries,
        cursor,
        next_seq,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("恢复不该失败")
}

/// 建一条边 → 快照 + 重放 → 反向 `await` 仍然被拒。
#[test]
fn a_would_be_cycle_is_still_rejected_after_a_snapshot_and_replay() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    let b = live.spawn_child(&root, ChildConfig::default(), None).unwrap();

    live.await_agent(&a, &b, AwaitUntil::Settled)
        .expect("A await B 该成功");

    let mut restored = restore_from(&live);

    // 等待图本身也该原样恢复回来。
    assert_eq!(
        restored.awaiting_on(&a),
        vec![(b.clone(), AwaitUntil::Settled)],
        "恢复之后 A 的等待边该原样还在"
    );

    // 核心断言：反向 await（B await A）在恢复出来的会话上仍然被挡。
    let denied = restored
        .await_agent(&b, &a, AwaitUntil::Settled)
        .expect_err("反向 await 该在恢复之后仍然被拒");
    assert!(
        matches!(denied, AwaitDenied::WouldCycle { .. }),
        "该是 WouldCycle，拿到 {denied:?}"
    );
    if let AwaitDenied::WouldCycle { chain } = &denied {
        assert!(chain.contains(&a), "链上该有 A：{chain:?}");
        assert!(chain.contains(&b), "链上该有 B：{chain:?}");
    }
}

/// 只重放日志、不给快照（`None`）——同一条断言在另一条恢复路径上也成立
/// （跟 `session_subagent_restore.rs::the_restored_tree_keeps_stepping_and_undoing`
/// 同一个取舍：两条恢复路径都要保证行为一致）。
#[test]
fn the_cycle_check_survives_log_only_replay_too() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    let b = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    live.await_agent(&a, &b, AwaitUntil::Settled).unwrap();

    let entries: Vec<AgentEntry> = live.history().entries().cloned().collect();
    let cursor = live.cursor();
    let next_seq = entries.last().map_or(0, |e| e.seq + 1);
    let mut restored = Session::restore(
        root,
        None,
        entries,
        cursor,
        next_seq,
        100,
        agent_core::AgentLimits::default(),
        &mut |_| {},
    )
    .expect("纯日志重放不该失败");

    let denied = restored
        .await_agent(&b, &a, AwaitUntil::Settled)
        .expect_err("纯日志重放之后反向 await 也该被拒");
    assert!(matches!(denied, AwaitDenied::WouldCycle { .. }));
}

/// 恢复出来的等待图不是一次性的快照：接着建立新的边、接着查环，一切照常。
#[test]
fn the_restored_wait_graph_keeps_working() {
    let mut live = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    let b = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    let c = live.spawn_child(&root, ChildConfig::default(), None).unwrap();
    live.await_agent(&a, &b, AwaitUntil::Settled).unwrap();

    let mut restored = restore_from(&live);

    // 恢复之后接着建一条新边（B await C），这条不成环，该放行。
    restored
        .await_agent(&b, &c, AwaitUntil::Settled)
        .expect("不成环的新边该放行");

    // 现在 A→B→C，C→A 会闭合成环，该被拒。
    let denied = restored
        .await_agent(&c, &a, AwaitUntil::Settled)
        .expect_err("三角环该被拒");
    assert!(matches!(denied, AwaitDenied::WouldCycle { .. }));
}
