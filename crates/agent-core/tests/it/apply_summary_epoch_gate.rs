//! Issue 107，红线 6 的正面战场：摘要回来时，`Session::step` 那道 105 已经建好
//! 的 epoch 闸（`effect_compact_epoch_gate.rs` 验过一次，只看「不产出 effect」）
//! 在这里要验得更狠一点——**`SendPlan` / `summary_text` 也必须一个字节不动**，
//! 不只是 effects 是空的。
//!
//! 场景造法照抄 `effect_compact_epoch_gate.rs` / `session_epoch_gate.rs`：
//! `Event::Cancel` 与 `undo_turn` 是真的会 bump 世代的两个动作，「在飞 → 世界
//! 变了 → 迟到的 CompactDone 带着旧世代回来」用它们造。
//!
//! 反向锁在最后一条：只测「过期被丢」挡不住一个「Compact 的回执一律丢弃」的
//! 实现——那样的实现在前两条测试里也会全绿。

use std::sync::Arc;

use agent_core::{AgentId, Effect, Epoch, Event, Notice, Session, TurnStatus};

use crate::support;
use crate::support::session::{new_session, session_with_pending_tools};

/// 一个已经 `Cancel` 过的会话——`Event::Cancel` 是 M1 就有的、真的会 bump 世代
/// 的动作。
fn cancelled_session() -> (Session, Epoch) {
    let mut s = new_session();
    let old = s.epoch();
    let _ = s.step(support::cancel_event());
    assert_eq!(s.epoch(), old.next(), "Cancel 必须 bump 世代（红线 6）");
    (s, old)
}

/// 摘要在飞时用户取消：迟到的 `CompactDone` 带着 cancel 之前的旧世代回来，
/// 不该写进 `SendPlan`。
#[test]
fn a_summary_that_arrives_after_cancel_in_flight_is_not_written() {
    let (mut s, old_epoch) = cancelled_session();
    let root = AgentId::root();
    let before_plan = s.send_plan_of(&root);

    let effects = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: Arc::from("属于一个已经被取消掉的世界的摘要"),
        epoch: old_epoch,
    });

    assert!(
        effects.is_empty(),
        "旧 epoch 的 CompactDone 不该产出任何 effect"
    );
    assert_eq!(
        s.send_plan_of(&root),
        before_plan,
        "旧 epoch 的摘要不该写进 SendPlan——边界和引用都不该动"
    );
}

/// 摘要在飞时用户 `/undo` 一次：同样的道理，迟到的摘要不该写入，`SendPlan`
/// 该跟 undo 之后的状态一致。
#[test]
fn a_summary_that_arrives_after_an_undo_is_not_written() {
    let mut s = session_with_pending_tools(&[("call_1", "srv:fs/read")]);
    let root = AgentId::root();
    let in_flight = s.epoch();

    let _ = s.undo_turn();
    assert_eq!(s.epoch(), in_flight.next(), "undo 必须 bump 世代（红线 6）");
    assert_eq!(s.status(), TurnStatus::Idle);
    let before_plan = s.send_plan_of(&root);

    let effects = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: Arc::from("在飞时用户 /undo 了一轮，这份摘要该被丢弃"),
        epoch: in_flight,
    });

    assert!(
        effects.is_empty(),
        "旧 epoch 的 CompactDone 不该产出任何 effect"
    );
    assert_eq!(
        s.send_plan_of(&root),
        before_plan,
        "SendPlan 该跟 undo 之后的状态一致，摘要不该悄悄写进去"
    );
}

/// 反向锁：epoch 对得上时，`step` 真的放行（能观察到
/// `Notice::CompactionSummaryReceived`），而不是「Compact 的回执一律丢弃」——
/// 只测上面两条「过期被丢」挡不住这种实现，因为丢弃信号（空 effects）跟
/// 「一律丢弃」在外面长得一模一样。
///
/// `step` 本身**不写** `SendPlan`——这是 105/107 定死的分工，写在
/// `command/apply_summary.rs` 模块文档「epoch 在哪校验」：`upto` 不在
/// `Event::CompactDone` 里（事件形状 105 定死，不带历史正文），所以回写的调用点
/// 在持有 `upto` 的那一方（108 接线）：先看到这条通报确认过闸，再直接调
/// `apply_summary`。这里把那一步接着做完，验的是**整条链路**：过闸 → 通报 →
/// 调用方拿着自己记住的 `upto` 写入 → 边界真的动了、`summary_text` 真的取得到。
#[test]
fn matching_epoch_lets_the_pipeline_observe_acceptance_and_then_apply_summary_writes() {
    let mut s = new_session();
    let root = AgentId::root();
    let live_epoch = s.epoch();
    // 调用方（108 接线的那一方）自己记住的、发起 Effect::Compact 时用的 upto——
    // 事件里不带这个数，107 的硬契约就是调用方得自己留着。
    let upto = 3;

    let summary_text: Arc<str> = Arc::from("当前世代的摘要，该被真的写进去");
    let effects = s.step(Event::CompactDone {
        agent: support::agent(),
        summary: summary_text.clone(),
        epoch: live_epoch,
    });

    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Emit(Notice::CompactionSummaryReceived))),
        "epoch 对得上，闸该放行并发一条 CompactionSummaryReceived 通报：{effects:?}"
    );
    assert!(
        s.send_plan_of(&root).is_pristine(),
        "step 这一步本身不写 SendPlan——写入是另一条命令，不是这道闸的职责"
    );

    let id = s.apply_summary(&root, upto, summary_text.clone()).unwrap();

    let plan = s.send_plan_of(&root);
    assert_eq!(plan.boundary(), upto, "调用方看到通报之后写入，边界真的动了");
    assert_eq!(plan.summary(), Some(&id));
    assert_eq!(
        s.summary_text(&root, &id),
        Some(summary_text),
        "写进去的摘要正文该原样取得到"
    );
}

/// `CompactFailed` 是正常事件不是异常路径（106 验收）：epoch 对得上的一条
/// `CompactFailed` 不该动 `SendPlan`——压缩这一次作废，边界不动。
#[test]
fn compact_failed_with_a_matching_epoch_never_touches_send_plan() {
    let mut s = new_session();
    let root = AgentId::root();
    let live_epoch = s.epoch();
    let before_plan = s.send_plan_of(&root);

    let _ = s.step(Event::CompactFailed {
        agent: support::agent(),
        epoch: live_epoch,
    });

    assert_eq!(
        s.send_plan_of(&root),
        before_plan,
        "CompactFailed 一律不写状态，边界不该动"
    );
}
