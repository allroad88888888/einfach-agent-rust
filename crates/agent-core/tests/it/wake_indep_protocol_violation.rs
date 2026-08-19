//! 214 独立验收 · 第 7 条：**在飞状态收到 `Event::Wake` 必须是协议违规，不能悄悄放行**。
//!
//! `Event::Wake` 只认「没在跑」的入口（`Idle` 与终态 `Done`/`Failed`，见
//! `docs/issues/214-wake-a-terminal-agent.md` §验收）。`Thinking` 与 `ToolsPending`
//! 是「在飞」的两态——它们正等着一个 provider 回执或工具回执，这时候被 core
//! 半路拉去发起一次新的 `CallProvider`，会跟那条已经在飞的请求打架（两次请求
//! 共用同一份 `Messages`，谁先回来谁的回执先落地就变成竞态）。
//!
//! 写错的形状是「转移表悄悄放行，状态被改写」——那是静默错值，不报错、只在
//! 高并发下偶发。这里钉死的是：状态一个字节不变、`history_len` 不多一条 entry
//! （非法转移不留 `Entry`，见 `command/transitions/mod.rs` 模块文档），并且
//! `effects` 里必须有一条 `Notice::ProtocolViolation` 携带正确的 `state`。
//!
//! 黑盒来源：`docs/issues/214-wake-a-terminal-agent.md` §验收（跳过「实做记录」）、
//! `agent_core::engine::event::Event::Wake` 的 rustdoc（`event.rs`，非禁读文件）、
//! `command/transitions/mod.rs` 里非法格「状态不变 + `ProtocolViolation`」的既有
//! 约定（`turn_status_terminal.rs` / `session_transitions_grid.rs` 同一批断言的
//! 先例）。**没有读** `command/transitions/wake.rs`。

use agent_core::{Effect, Event, Notice, TurnStatus};

use crate::support::session::{observe, session_at};

/// 在飞的两态各自收到 `Wake` 都该被拒——同一条断言跑两次，两态各自独立判定。
#[test]
fn in_flight_statuses_reject_wake_as_a_protocol_violation() {
    for status in [TurnStatus::Thinking, TurnStatus::ToolsPending] {
        let mut session = session_at(&status);
        let before = observe(&session);

        let effects = session.step(Event::Wake {
            agent: session.agent().clone(),
            epoch: session.epoch(),
        });

        assert_eq!(
            session.status(),
            status,
            "{status:?} 收到 Wake 之后状态必须一个字节不变"
        );
        assert_eq!(
            observe(&session),
            before,
            "{status:?}：非法转移不该动任何 primitive / epoch / turn_id / 日志长度"
        );
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::Emit(Notice::ProtocolViolation { state, .. }) if *state == status
            )),
            "{status:?} 该报一条携带正确 state 的 ProtocolViolation：{effects:?}"
        );
    }
}
