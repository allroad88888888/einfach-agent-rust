//! 205 独立测试（三）：收件箱**白拿的那套机制**——undo 连带撤销且不产生屏障、
//! 落盘往返带着时机标记回来、`Private` 决定谁读得到。
//!
//! 黑盒来源：docs/issues/205-core-peek-and-inbox.md「验收」、
//! docs/issues/204-agent-mesh-decision.md §一/§二/§四、
//! docs/INVARIANTS.md 红线 3（primitive 可序列化）/ 红线 10（跨 agent 读不限方向，
//! `Private` 是唯一的闸）/ 红线 11。**实现体一行没读**（见 `inbox_indep.rs` 顶部）。

use std::sync::Arc;

use crate::inbox_indep::{last_message_text, tree};
use crate::support::{provider_done_end_turn, user_input_event};
use agent_core::{
    AgentLimits, AgentValue, AtomKey, DEFAULT_HISTORY_CAP, Deliver, ReadDenied, Session, Slot,
    UndoReport, Undoability,
};

/// 一次 `deliver` + 一次 `drain_now` 之后 `/undo` 那一轮：收件箱与 `Messages`
/// **都回到投递之前**，而且两条都是 `StateOnly`——没碰外部世界，不该产生屏障。
#[test]
fn undoing_the_delivering_turn_restores_both_sides_without_a_barrier() {
    let (mut session, root, a1, _a2) = tree();
    let _ = session.step(user_input_event("第一轮"));
    let _ = session.step(provider_done_end_turn(session.epoch(), "答完了"));

    session.begin_turn();
    let inbox_before = session.inbox_of(&a1);
    let messages_before = session.messages_of(&a1);
    assert!(inbox_before.is_empty());

    session
        .deliver(&root, &a1, Arc::from("中途纠偏"), Deliver::Now)
        .expect("父→子");
    assert_eq!(
        session.history().last().unwrap().meta.undoability,
        Undoability::StateOnly,
        "投递没碰外部世界，不该是屏障"
    );
    assert_eq!(session.drain_now(&a1), 1);
    assert_eq!(
        session.history().last().unwrap().meta.undoability,
        Undoability::StateOnly,
        "排空同理"
    );
    assert_ne!(session.messages_of(&a1), messages_before, "状态真的动了");

    match session.undo_turn() {
        UndoReport::Applied { .. } => {}
        other => panic!("两条 StateOnly 不该拦住 undo，得到 {other:?}"),
    }

    assert_eq!(session.inbox_of(&a1), inbox_before, "收件箱回到投递之前");
    assert_eq!(
        session.messages_of(&a1),
        messages_before,
        "Messages 也回到投递之前"
    );
}

/// **落盘往返带时机标记**：`Now` 与 `NextTurn` 各一条 → 存盘 → 恢复 →
/// 收件箱内容**含 `when`** 逐条相同。丢了标记 = 一条该等下一轮的消息当场被灌进去，
/// 所以最后再验一次「恢复之后两档仍然分得开」。
#[test]
fn a_snapshot_round_trip_keeps_the_delivery_mark_of_every_item() {
    let (mut session, root, a1, _a2) = tree();
    session
        .deliver(&a1, &root, Arc::from("本轮"), Deliver::Now)
        .expect("Now");
    session
        .deliver(&a1, &root, Arc::from("下一轮"), Deliver::NextTurn)
        .expect("NextTurn");
    let before = session.inbox_of(&root);
    assert_eq!(
        before.iter().map(|i| i.when).collect::<Vec<_>>(),
        vec![Deliver::Now, Deliver::NextTurn]
    );

    // 红线 3 + 11：值可序列化，而且往返之后逐字节相同。
    let snap = session.primitives();
    let json = serde_json::to_string(&snap).expect("primitives 该能序列化（红线 3）");
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&json).expect("也该能读回来");
    assert_eq!(back, snap);
    assert_eq!(
        serde_json::to_string(&back).unwrap(),
        json,
        "往返之后逐字节相同"
    );

    // 公开的恢复面：日志回放（`None` 快照 = 从头重放全部日志）。
    let entries: Vec<_> = session.history().entries().cloned().collect();
    let cursor = session.cursor();
    let next_seq = entries.last().map(|e| e.seq + 1).unwrap_or(0);
    let mut restored = Session::restore(
        root.clone(),
        None,
        entries,
        cursor,
        next_seq,
        DEFAULT_HISTORY_CAP,
        AgentLimits::default(),
        &mut |k| panic!("恢复遇到不认识的键 {k:?}"),
    )
    .expect("恢复不该拒绝一份自己刚生成的落盘件");

    assert_eq!(
        restored.inbox_of(&root),
        before,
        "收件箱逐条相同——`from` / `text` / `when` 都算在内"
    );
    assert_eq!(restored.primitives(), session.primitives());

    // 标记真的还在起作用，而不只是长得一样。
    assert_eq!(restored.drain_now(&root), 1, "恢复之后仍然只搬得走 Now 那条");
    let left = restored.inbox_of(&root);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].when, Deliver::NextTurn);
    assert!(last_message_text(&restored, &root).ends_with("本轮"));
}

/// 横读全开（决策 35）：兄弟的 `Status` 读一次就拿到值。
#[test]
fn a_sibling_status_is_readable() {
    let (session, _root, a1, a2) = tree();

    let value = session
        .read_agent(&a2, Slot::Status)
        .expect("兄弟的 Status 该读得到");
    assert!(value.as_status().is_some(), "拿到的该是一个状态值：{value:?}");

    // 反过来也一样——方向不再是判据。
    assert!(session.read_agent(&a1, Slot::Status).is_ok());
}

/// **发得进去 ≠ 读得出来**：a2 刚往 a1 的收件箱投过一条，照样读不到那个收件箱。
/// `Inbox` 站 `Private` 是刻意的——一旦 `Shared`，「谁给谁发过什么」就变成
/// 任何人都能订阅的响应式依赖（204 §五 点名不做）。
#[test]
fn the_inbox_is_private_even_to_the_agent_that_just_delivered_into_it() {
    let (mut session, _root, a1, a2) = tree();
    session
        .deliver(&a2, &a1, Arc::from("兄弟一句"), Deliver::Now)
        .expect("投得进去");

    for slot in [Slot::Inbox, Slot::TurnsUsed, Slot::Summaries] {
        assert!(
            matches!(
                session.read_agent(&a1, slot),
                Err(ReadDenied::NotVisible { .. })
            ),
            "{slot:?} 是这个 agent 的内部账本，跨 agent 读不到"
        );
    }

    assert_eq!(
        session.inbox_of(&a1).len(),
        1,
        "自读照旧——`Private` 说的是**别的** agent 读不到"
    );
}
