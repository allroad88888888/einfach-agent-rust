//! 214 独立验收 · 第 8 条：**没在跑但没有可发的料 → 唤醒什么都不做**。
//!
//! `Idle` 是「没在跑」的两态之一（另一档是终态 `Done`/`Failed`，见
//! `docs/issues/214-wake-a-terminal-agent.md` §验收「入口是没在跑的两态」）。
//! 一个刚建好、`Messages` 还是空的会话收到 `Event::Wake`：它不是非法转移
//! （`Idle` 本身合法），但也没有话可发——`Effect::CallProvider` 发出去只会让
//! provider 对着一段空历史打一次白付费的招呼。
//!
//! 这条跟撞顶（`wake_indep_turn_cap.rs`）是同一类「合法但什么都不做」的分支，
//! 差别只是原因：那条是预算耗尽，这条是压根没有话可发。两条都不该留下
//! `Entry`（`History::append` 拒绝空步——不写 primitive 就是空 batch），也都
//! **不是** `Notice::ProtocolViolation`——`Idle` 不在非法格里，报一条协议违规
//! 会把「正常但无事可做」跟「用错了协议」混成一件事。
//!
//! 黑盒来源：`docs/issues/214-wake-a-terminal-agent.md` §验收、
//! `command/transitions/mod.rs` 「非法格不写 primitive ⇒ 空 changes ⇒
//! `History::append` 拒绝空步」的既有约定（非禁读文件）。**没有读**
//! `command/transitions/wake.rs`。

use agent_core::{Effect, Event, Notice, Session, TurnStatus};

use crate::support::agent;

#[test]
fn an_idle_session_with_no_messages_produces_nothing_on_wake() {
    let mut session = Session::new(agent());
    assert_eq!(session.status(), TurnStatus::Idle, "前提：刚建的会话是 Idle");
    assert!(
        session.messages().is_empty(),
        "前提：`Messages` 是空的，否则这条测的是别的分支"
    );

    let before_history_len = session.history_len();
    let before_epoch = session.epoch();

    let effects = session.step(Event::Wake {
        agent: agent(),
        epoch: session.epoch(),
    });

    assert!(
        effects.is_empty(),
        "没有话可发，不该产出任何 effect（包括 CallProvider 和任何 Notice）：{effects:?}"
    );
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Emit(Notice::ProtocolViolation { .. }))),
        "Idle 是合法入口，这不该被判成协议违规"
    );
    assert_eq!(
        session.status(),
        TurnStatus::Idle,
        "状态该原地不动，不该被当成撞了什么顶"
    );
    assert_eq!(
        session.history_len(),
        before_history_len,
        "不写 primitive ⇒ 不留 entry"
    );
    assert_eq!(session.epoch(), before_epoch, "Wake 不 bump 世代");
}
