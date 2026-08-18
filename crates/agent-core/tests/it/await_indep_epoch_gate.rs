//! 212 验收「红线 6」：`await` 挂着时 `/undo` bump epoch → 目标后到的状态变化
//! 不会把一个已经作废的槽写活。
//!
//! `await` 落地之后，等待方那个工具槽跟别的工具槽没有任何区别——它是一格
//! 普通的 `Pending`，收敛靠一条 `Event::ToolResult`（或 `ToolFailed`），这条
//! 事件一样要过 `Session::step` 的 epoch 闸。这份测试直接喂那条"迟到的回执"，
//! 照 `session_indep_epoch_gate.rs` 的既有写法：undo 之后带着**旧** epoch 的
//! `ToolResult` 必须被静默丢弃，不产生任何 effect、不改任何 primitive。

use std::sync::Arc;

use agent_core::value::awaiting::AwaitUntil;
use agent_core::{AgentId, ChildConfig, Event, Session, ToolCallId, UndoReport};

use crate::support::{provider_done_tool_use_for, user_input_for};

const AWAIT_CALL: &str = "call_await_epoch";

#[test]
fn a_tool_result_synthesized_before_an_undo_is_dropped_after_it() {
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();
    let a = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();
    let b = session
        .spawn_child(&root, ChildConfig::default(), None)
        .unwrap();

    // 把 A 推到 ToolsPending：手上有一个待收敛的 `srv:agent/await` 槽——跟
    // runtime 真的截获一次 `await` 调用之后，A 该处在的那个状态逐位相同
    // （工具槽是否 Pending 只看 `Slot::ToolSlots` 的值，不看是谁写进去的）。
    let _ = session.step(user_input_for(&a, "干活"));
    let _ = session.step(provider_done_tool_use_for(
        &a,
        session.epoch(),
        &[(AWAIT_CALL, "srv:agent/await")],
    ));

    // 等待边本身也建起来——跟 runtime 的截获逻辑一样，这两件事（占住工具槽、
    // 建立等待边）是同一次调用的两个后果。
    session.begin_turn();
    session
        .await_agent(&a, &b, AwaitUntil::Settled)
        .expect("A await B 该成功");

    let old_epoch = session.epoch();
    let before = session.primitives();

    // undo 掉刚才建边那一轮——红线 6：undo 必须 bump epoch。
    let report = session.undo_turn();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    assert_ne!(session.epoch(), old_epoch, "undo 必须 bump epoch（红线 6）");

    let state_after_undo = session.primitives();

    // B 后来真的 Settled 了：runtime 的 harvest 会拿 `old_epoch` 合成一条
    // `Event::ToolResult` 喂回 A 那个槽——这里直接构造同一条事件，模拟「回执
    // 迟到」。
    let effects = session.step(Event::ToolResult {
        agent: a.clone(),
        epoch: old_epoch,
        call_id: ToolCallId::new(AWAIT_CALL),
        content: Arc::from("{\"reached\":true}"),
    });

    assert!(
        effects.is_empty(),
        "旧 epoch 的 await 回执必须被闸挡掉，不产出任何 effect"
    );
    assert_eq!(
        session.primitives(),
        state_after_undo,
        "被挡掉的回执不能改动任何 primitive——包括 A 那个已经作废的工具槽"
    );
    assert_ne!(
        session.primitives(),
        before,
        "对照组：undo 本身确实改过状态（负例检查，免得上面两条断言都是空对空）"
    );

    // 再确认一次工具槽的具体值：它该还是 undo 之后的样子（不会因为这条迟到
    // 回执又冒出一个"写活"的值）。`ToolSlots` 是 `Private`（跨 agent 读不到，
    // `visibility.rs`），所以不能走 `read_descendant`——直接从 `primitives()`
    // 这份完整状态转储里取同一个键（跟上面 `before`/`state_after_undo` 同一条
    // 取数路径，审计/快照走的正是这条不受 `Visibility` 约束的口）。
    let key = agent_core::AtomKey::Agent(a.clone(), agent_core::Slot::ToolSlots);
    let slots_now = session
        .primitives()
        .into_iter()
        .find(|(k, _)| k == &key)
        .map(|(_, v)| v)
        .expect("A 的 ToolSlots 键该在完整状态转储里");
    let slots_after_undo = state_after_undo
        .iter()
        .find(|(k, _)| k == &key)
        .map(|(_, v)| v.clone())
        .expect("undo 之后的快照里该有这一项");
    assert_eq!(slots_now, slots_after_undo, "工具槽的值该跟 undo 之后完全一致");
}
