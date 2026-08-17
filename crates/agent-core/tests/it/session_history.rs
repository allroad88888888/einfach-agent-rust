//! 026 验收：**一轮完整对话（mock 事件）之后，`history.len()` == 该轮 primitive
//! 写入的 batch 数，每条 `Entry` 的 `prev`/`next` 与转移语义吻合。**
//!
//! M1 没有对应文件——它压根没有日志。这一份和 `session_undo_redo.rs` 是 026 新增
//! 的那两块：一个证明「写下来了」，一个证明「退得回去」。

use crate::support;
use agent_core::{AgentValue, AtomKey, Slot, TurnStatus, Undoability};

use crate::support::session::new_session;

/// 一轮完整对话：user → provider(ToolUse) → tool result → provider(EndTurn)。
/// 四个事件、四次真的改了状态的转移 → **恰好四条 entry**，label 一一对上。
#[test]
fn one_full_turn_leaves_exactly_one_entry_per_transition_that_changed_something() {
    let mut s = new_session();

    let _ = s.step(support::user_input_event("读一下 a.txt"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "内容"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "读完了"));

    assert_eq!(s.status(), TurnStatus::Done { truncated: false });
    let labels: Vec<&str> = s.history().entries().map(|e| e.meta.label).collect();
    assert_eq!(
        labels,
        vec![
            "user_input",
            "provider_done",
            "tool_result",
            "provider_done"
        ]
    );
    assert_eq!(s.history_len(), 4);
    assert_eq!(s.cursor(), 4, "游标在栈顶");

    // 整轮的 entry 属于同一个 turn，`epoch` 全程没变（这一轮没有取消也没有 undo）。
    assert!(s.history().entries().all(|e| e.meta.turn_id == 1));
    assert!(
        s.history()
            .entries()
            .all(|e| e.meta.epoch == agent_core::Epoch::START)
    );
    assert!(
        s.history()
            .entries()
            .all(|e| e.meta.undoability == Undoability::StateOnly)
    );
}

/// 协议违规**不落条目**：它一个 primitive 都没写，`History::append` 拒绝空步。
/// 「状态不变」在日志这一侧同样是结构事实——undo 栈里不会出现按一下没反应的幽灵步。
#[test]
fn a_protocol_violation_does_not_leave_a_ghost_step() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("hi"));
    let before = s.history_len();

    // `Thinking` 收到 `UserInput` = 非法。
    let _ = s.step(support::user_input_event("再说一句"));

    assert_eq!(s.history_len(), before);
}

/// 每条 `Change` 的 `prev`/`next` 与转移语义吻合：拿 `user_input` 那一条逐项验。
///
/// 它该改的恰好是四个槽位——消息号计数器、消息历史、本轮已用轮数、状态；
/// **不该**碰前缀镜像、工具槽、两个上限。多一项少一项都是 undo 时的错值来源。
#[test]
fn the_changes_of_a_single_transition_match_the_transition_semantics() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("你好"));

    let entry = s.last_entry().expect("应该有一条 entry");
    assert_eq!(entry.meta.label, "user_input");

    let mut touched: Vec<Slot> = entry
        .changes
        .iter()
        .map(|c| match &c.key {
            AtomKey::Agent(_, slot) => *slot,
            other => panic!("M2 只写 Agent 槽位，收到 {other:?}"),
        })
        .collect();
    touched.sort();
    touched.dedup();
    assert_eq!(
        touched,
        vec![
            Slot::Messages,
            Slot::Status,
            Slot::NextMessageId,
            Slot::TurnsUsed
        ]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
    );

    for change in &entry.changes {
        let AtomKey::Agent(_, slot) = &change.key else {
            unreachable!()
        };
        match slot {
            Slot::Status => {
                assert_eq!(change.prev, AgentValue::Status(TurnStatus::Idle));
                assert_eq!(change.next, AgentValue::Status(TurnStatus::Thinking));
            }
            Slot::NextMessageId => {
                assert_eq!(change.prev, AgentValue::U64(1));
                assert_eq!(change.next, AgentValue::U64(2));
            }
            Slot::TurnsUsed => {
                assert_eq!(change.prev, AgentValue::U64(0));
                assert_eq!(change.next, AgentValue::U64(1));
            }
            Slot::Messages => {
                assert_eq!(change.prev.as_messages().unwrap().len(), 0);
                assert_eq!(change.next.as_messages().unwrap().len(), 1);
            }
            other => panic!("user_input 不该碰 {other:?}"),
        }
    }
}

/// derived 的重算**不产生** `Entry`——只有源状态进日志（009 的结构性事实）。
/// 这里从 agent 侧再钉一次：`tools_converged` 明明从 false 翻到 true，
/// 日志里却只有 primitive 那几条。
#[test]
fn recomputing_the_derived_produces_no_entry() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("hi"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));
    assert!(!s.tools_converged());

    let recomputes = s.debug_recompute_count();
    let entries_before = s.history_len();
    let _ = s.step(support::tool_result_event(s.epoch(), "call_1", "ok"));

    assert!(s.tools_converged());
    assert!(s.debug_recompute_count() > recomputes, "derived 确实重算了");
    assert_eq!(
        s.history_len(),
        entries_before + 1,
        "只多了 tool_result 那一条"
    );
    assert!(
        s.last_entry()
            .unwrap()
            .changes
            .iter()
            .all(|c| matches!(c.key, AtomKey::Agent(_, _))),
        "日志里只有源状态的键"
    );
}

/// 会话级命令同样留痕：`begin_turn` 与 `set_max_turns` 都是 primitive 写入，
/// 各留一条可回滚的 entry（红线 2 对它们一视同仁）。
#[test]
fn session_level_commands_are_logged_like_any_other_write() {
    let mut s = new_session();
    s.set_max_turns(7);
    assert_eq!(s.last_entry().unwrap().meta.label, "set_max_turns");

    let _ = s.step(support::user_input_event("hi"));
    let _ = s.step(support::provider_done_end_turn(s.epoch(), "ok"));
    s.begin_turn();

    let labels: Vec<&str> = s.history().entries().map(|e| e.meta.label).collect();
    assert_eq!(
        labels,
        vec!["set_max_turns", "user_input", "provider_done", "begin_turn"]
    );
    // 新一轮的 entry 归新 turn，旧的归旧 turn——`undo_turn` 的分组依据。
    let turns: Vec<u64> = s.history().entries().map(|e| e.meta.turn_id).collect();
    assert_eq!(turns, vec![1, 1, 1, 2]);
}
