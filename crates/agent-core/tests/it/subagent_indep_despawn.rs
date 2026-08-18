//! 028 独立测试：`despawn_child` 的 019 三约束跨 agent 版——teardown 把子的
//! 全部活值记成 `prev`、自叶向根逐出不 panic、undo 之后子树值完整重建并且
//! 接着能用。
//!
//! 黑盒来源：docs/issues/028-multi-agent-graph.md §3、docs/STATE-MODEL.md
//! §「evict 与 undo」、docs/issues/019-applier-recreate.md、cargo doc 的
//! `command::despawn` 模块文档。不读 `src/command/despawn.rs` 源码。
//!
//! # 分歧：「被外部读依赖时拒绝」（`DespawnRefused::StillRead`）没有在这里测
//!
//! 尝试过，结论是黑盒范围内造不出这个场景，如实记在这里而不是假装测到：
//!
//! - `Session::read_ancestor`/`read_descendant` 是一次性的 `peek`
//!   （cross_read 模块文档原文：「读口是非创建的」），在测试代码里直接调用
//!   （不在任何 derived 的求值上下文里）不会挂上任何订阅——这类原子引擎的依赖
//!   追踪只在"当前正在求值的 derived"存在时才会记边，测试代码本身不是一个
//!   derived。
//! - 唯一会创建跨 agent 订阅边的入口是构图函数（`create_derived`），029 之前
//!   还没有任何生产代码用它做跨 agent 汇聚（`tools_converged` 只读本 agent自己
//!   的 `ToolSlots`）——issue 028「推给 029 的」第 5 条明确说这类汇聚 derived
//!   是 029 才会长出来的东西。
//! - `Session` 也没有公开面能让宿主手工建一条跨 agent 的订阅边。
//!
//! 所以 `StillRead` 目前只能由实现方自己在 crate 内部的白盒单测里通过私有
//! 钩子人为造出来（`src/command/despawn.rs` 的内联测试，本次禁止读取）。黑盒
//! 测试改为验证约束 2 的正向面：一棵没有外部读者的真实子树，despawn 该顺利
//! 通过而不会被状态驱动的逐出闸误伤（下面 `despawn_evicts_leaf_first_...`）。

use crate::support::session::new_session;
use crate::support::{provider_done_tool_use_for, tool_result_for, user_input_for};
use agent_core::{
    AgentEntry, AgentId, AgentValue, AtomKey, ChildConfig, Session, Slot, TurnStatus, UndoReport,
};

fn child_key_count(session: &Session, agent: &AgentId) -> usize {
    session
        .primitives()
        .iter()
        .filter(|(k, _)| k.agent() == agent)
        .count()
}

fn value_of(session: &Session, agent: &AgentId, slot: Slot) -> AgentValue {
    session
        .primitives()
        .into_iter()
        .find(|(k, _)| *k == AtomKey::Agent(agent.clone(), slot))
        .map(|(_, v)| v)
        .unwrap_or(AgentValue::Null)
}

fn prev_of(entry: &AgentEntry, agent: &AgentId, slot: Slot) -> AgentValue {
    entry
        .changes
        .iter()
        .find(|c| c.key == AtomKey::Agent(agent.clone(), slot))
        .unwrap_or_else(|| panic!("teardown entry 缺 {slot:?} 的 change"))
        .prev
        .clone()
}

fn next_of(entry: &AgentEntry, agent: &AgentId, slot: Slot) -> AgentValue {
    entry
        .changes
        .iter()
        .find(|c| c.key == AtomKey::Agent(agent.clone(), slot))
        .unwrap_or_else(|| panic!("teardown entry 缺 {slot:?} 的 change"))
        .next
        .clone()
}

/// 把子 agent 驱动出真实活值：Messages 非空、Status = ToolsPending、有一个
/// 挂起的工具槽——这样 despawn 的自叶向根顺序才有真正要处理的东西（子自己的
/// `tools_converged` derived 真的订阅着 `ToolSlots`，顺序反了会撞上 019 记录
/// 的那个 panic）。
fn spawn_and_drive_child(session: &mut Session) -> AgentId {
    let root = session.agent().clone();
    let child = session
        .spawn_child(
            &root,
            ChildConfig {
                tools_allowed: vec!["srv:fs/read".into()],
                ..ChildConfig::default()
            },
            None,
        )
        .expect("spawn child");
    session.step(user_input_for(&child, "do something"));
    session.step(provider_done_tool_use_for(
        &child,
        session.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    assert_eq!(
        value_of(session, &child, Slot::Status),
        AgentValue::Status(TurnStatus::ToolsPending)
    );
    child
}

#[test]
fn the_teardown_entry_records_every_live_value_as_prev_and_the_default_as_next() {
    let mut session = new_session();
    let child = spawn_and_drive_child(&mut session);

    let live_messages = value_of(&session, &child, Slot::Messages);
    let live_status = value_of(&session, &child, Slot::Status);
    let live_slots = value_of(&session, &child, Slot::ToolSlots);
    assert_ne!(
        live_messages,
        Slot::Messages.default_value(),
        "子该已经写过至少一条消息"
    );

    let _report = session
        .despawn_child(&child)
        .expect("despawn should succeed");

    let entry = session.last_entry().expect("despawn 应该留下一条 entry");

    assert_eq!(prev_of(entry, &child, Slot::Messages), live_messages);
    assert_eq!(prev_of(entry, &child, Slot::Status), live_status);
    assert_eq!(prev_of(entry, &child, Slot::ToolSlots), live_slots);

    assert_eq!(
        next_of(entry, &child, Slot::Messages),
        Slot::Messages.default_value()
    );
    assert_eq!(
        next_of(entry, &child, Slot::Status),
        Slot::Status.default_value()
    );
    assert_eq!(
        next_of(entry, &child, Slot::ToolsAllowed),
        AgentValue::Null,
        "ToolsAllowed 移出活名单"
    );
}

#[test]
fn despawn_evicts_leaf_first_without_panicking_and_leaves_exactly_one_tombstone() {
    let mut session = new_session();
    let child = spawn_and_drive_child(&mut session);
    // 槽位数 = `Slot::ALL.len()`（每个 agent 一份，`build_agent` 不给 root 开小灶）：
    // 028 是 10、039 加 `SkillsActive` 是 11、073 加 `HostTools` 是 12、
    // 064 加 `HostSkills` 是 13、076 加 `DisabledBuiltins` 是 14、
    // 093 加 `ExecutionProfile` 是 15、100 加 `SendPlan` 是 16、
    // 103 加 `PrevSendPlan` 是 17、107 加 `Summaries` 是 18、
    // 134 加 `PrefixChunks` 是 19、144 加 `PrefixAllowed` 是 20、
    // 154 加 `HostPrefix` 是 21，205 加 `Inbox`（决策 35）是 22、
    // 209 加 `Notes`（决策 35 §三）是 23、212 加 `AwaitingOn` 是 24。
    assert_eq!(child_key_count(&session, &child), 24);

    let report = session
        .despawn_child(&child)
        .expect("despawn should not panic or refuse");

    assert_eq!(report.agents, vec![child.clone()]);
    assert_eq!(
        report.atoms_evicted, 23,
        "二十四个槽位里只留 ToolsAllowed 一个墓碑（144 的 PrefixAllowed、154 的\
         HostPrefix、205 的 Inbox、209 的 Notes、212 的 AwaitingOn 都不是墓碑，照样\
         被逐出——despawn.rs 只特化 ToolsAllowed 一个
         变体）"
    );
    assert_eq!(
        child_key_count(&session, &child),
        1,
        "其余二十个 atom 该被物理逐出"
    );
    assert!(!session.is_live(&child));
    assert_eq!(
        value_of(&session, &child, Slot::ToolsAllowed),
        AgentValue::Null
    );
}

#[test]
fn undo_after_despawn_rebuilds_the_subtree_with_its_live_values_and_it_keeps_working() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = spawn_and_drive_child(&mut session);

    // 把 despawn 隔进它自己的 root turn，undo_turn 才只回退这一条命令，不会
    // 顺带把子之前写状态的那些 entry 也一起弹掉（begin_turn 只动 root 自己
    // 的三个槽位，子的 ToolsPending 状态不受影响）。
    session.begin_turn();
    let _report = session.despawn_child(&child).expect("despawn");
    assert!(!session.is_live(&child));

    let undo = session.undo_turn();
    assert!(
        matches!(undo, UndoReport::Applied { .. }),
        "undo 该 Applied，实际 {undo:?}"
    );

    assert!(
        session.is_live(&child),
        "undo 一次 despawn 之后子该重新活着"
    );
    assert_eq!(session.children_of(&root), vec![child.clone()]);
    assert_eq!(
        child_key_count(&session, &child),
        24,
        "全部二十四个槽位都该被按需重建（144 加了 PrefixAllowed，154 加了 HostPrefix，\
         205 加了 Inbox，209 加了 Notes，212 加了 AwaitingOn）"
    );
    assert_eq!(
        value_of(&session, &child, Slot::Status),
        AgentValue::Status(TurnStatus::ToolsPending)
    );

    // 子接着工作：喂它那条挂起工具调用的结果，让它收敛。注意这里不能拿
    // `history_len()` 前后一比：undo 之后游标不在栈顶，这一步新写入会按
    // STATE-MODEL 的默认语义覆盖掉刚刚被 undo 悬空的那条 redo 尾（despawn 的
    // teardown entry），一条换一条，条数可能不变——`cursor()` 前进 + 状态真的
    // 推进了才是「继续工作」的证据。
    let before_cursor = session.cursor();
    session.step(tool_result_for(&child, session.epoch(), "call_1", "ok"));
    assert!(
        session.cursor() > before_cursor,
        "游标该往前走，证明这一步是被正常接受、记账的"
    );
    assert_eq!(
        value_of(&session, &child, Slot::Status),
        AgentValue::Status(TurnStatus::Thinking),
        "唯一的挂起槽收敛之后子该继续推进到下一轮 Thinking（不是停在原地）"
    );
}
