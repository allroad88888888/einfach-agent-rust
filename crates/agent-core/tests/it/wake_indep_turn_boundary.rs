//! 214 独立验收 · 第 6 条：**唤醒不开新 turn，`/undo` 一次退掉整轮连带唤醒后的活**。
//!
//! `docs/issues/214-wake-a-terminal-agent.md` §一 拍板「属于当前 turn，不开新
//! turn」：子 agent 本来就在父的同一次 turn 里生死，唤醒只是「同一轮里再动一次」，
//! 不是「开一轮新的」——`turn_id` 是 `undo_turn` 的分组依据（`Session::begin_turn`
//! 的文档），藏进一格转移里就等于让转移表偷偷决定「一轮从哪开始」，这正是 214
//! §缘起 说明它不能靠放宽 `on_user_input` 的闸来做的理由：`Session::begin_turn`
//! 才是唯一合法的开新轮入口，唤醒不调它。
//!
//! 这里钉两件事：
//! 1. `turn_id` 唤醒前后逐字节相同；
//! 2. 唤醒之后接着完成那次回合（喂一条 `ProviderDone` 收尾），整个过程
//!    （spawn 发信人、投递、唤醒、唤醒后那次回复）都落在同一个 `turn_id` 上，
//!    于是一次 `Session::undo_turn()` 能把它们**全部**退回会话开始之前——不是
//!    「退掉一部分」或者「因为唤醒另起了一个 turn 而退不干净」。
//!
//! 黑盒来源：`docs/issues/214-wake-a-terminal-agent.md` §一 / §验收、
//! `command/session.rs` 中 `Session::begin_turn` 的 rustdoc（非禁读文件，「只有
//! root 开新一轮」「子 agent 的 entry 继承所在 root turn 的 turn_id」）、
//! `command/undo.rs` 中 `Session::undo_turn` 的 rustdoc。**没有读**
//! `command/transitions/wake.rs`。

use std::sync::Arc;

use agent_core::{ChildConfig, Deliver, Effect, Event, Session, TurnStatus, UndoReport};

use crate::support::{agent, provider_done_end_turn};

#[test]
fn wake_keeps_the_turn_id_and_undo_turn_retracts_everything_it_did() {
    let root = agent();
    let mut session = Session::new(root.clone());

    // 一次最简单的问答，让 root 落终态（turns_used = 1 就够，这条测的是 turn
    // 边界不是预算计数——那条另有 `wake_indep_wakes_and_counts.rs` 钉着）。
    let _ = session.step(Event::UserInput {
        agent: root.clone(),
        text: Arc::from("先聊一句"),
    });
    let _ = session.step(provider_done_end_turn(session.epoch(), "第一次答完了"));
    assert_eq!(session.status(), TurnStatus::Done { truncated: false });

    let sender = session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("spawn 一个发信人");
    session
        .deliver(&sender, &root, Arc::from("TURNBOUNDARY-该被唤醒读到"), Deliver::Now)
        .expect("投递该成功");
    let moved = session.drain_now(&root);
    assert_eq!(moved, 1);

    let turn_before_wake = session.turn_id();

    let effects = session.step(Event::Wake {
        agent: root.clone(),
        epoch: session.epoch(),
    });
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::CallProvider { .. })),
        "前提：唤醒真的发起了一次 CallProvider，否则下面的比较是空跑的"
    );
    assert_eq!(
        session.turn_id(),
        turn_before_wake,
        "唤醒发出 CallProvider 那一刻，turn_id 就不该变"
    );

    // 完成这次被唤醒之后的回合。
    let _ = session.step(provider_done_end_turn(session.epoch(), "被叫醒之后又答了一句"));
    assert_eq!(session.status(), TurnStatus::Done { truncated: false });
    assert_eq!(
        session.turn_id(),
        turn_before_wake,
        "唤醒之后接着完成的这次回合，仍然是同一个 turn_id——214 不开新 turn"
    );

    // 一次 `/undo` 该把 spawn / deliver / drain_now / wake 之后那次回复
    // 全部退回会话开始之前——它们全部落在同一个 turn_id 上。
    let report = session.undo_turn();
    match report {
        UndoReport::Applied { turn_id, .. } => {
            assert_eq!(turn_id, turn_before_wake, "退的该是唤醒所在的那个 turn")
        }
        other => panic!("这一轮全是纯状态操作，不该被屏障拦下：{other:?}"),
    }

    assert_eq!(
        session.cursor(),
        0,
        "游标 = 已生效条数（history_len 含被 undo 掉、还能 redo 回来的尾巴，不是这里该比的数）：\
         spawn/deliver/drain_now/两次回复全在同一个 turn 里，该被退空"
    );
    assert!(
        !session.is_live(&sender),
        "连带把这一轮 spawn 出来的发信人也退掉了"
    );
    assert_eq!(
        session.status(),
        TurnStatus::Idle,
        "回到会话刚建好、还没进过第一轮的状态"
    );
    assert_eq!(session.turns_used(), 0, "预算计数也一并退回 0");
    assert!(
        session.inbox_of(&root).is_empty(),
        "投递也被退掉了，收件箱回到空"
    );
}
