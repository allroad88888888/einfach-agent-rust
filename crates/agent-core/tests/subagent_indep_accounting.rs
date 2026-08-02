//! 028 独立测试：子 agent 的记账归属——entry 的 `turn_id` 与 root 当轮一致
//! （决策 5 的实检）。
//!
//! 黑盒来源：docs/ROADMAP.md 决策 5、docs/STATE-MODEL.md §「Command log」
//! （「turn_id 由 root agent 分配，子 agent 的所有 entry 继承所在 root turn
//! 的 turn_id」）、cargo doc 的 `EntryMeta`/`Session::begin_turn` 文档。
//!
//! `EntryMeta` 没有单独的 `agent` 字段（只有 `turn_id`/`epoch`/`label`/
//! `barrier`），归属要通过 `entry.changes` 里 `AtomKey::agent()` 反查——这就是
//! STATE-MODEL 说的「`agent` 仅用于 UI 时间线显示，不参与 undo 判定」在这份
//! 落地代码里的具体形状。

mod support;

use agent_core::{AgentId, ChildConfig, Session};
use support::session::new_session;
use support::{provider_done_end_turn_for, user_input_for};

fn turn_ids_touching(session: &Session, agent: &AgentId) -> Vec<u64> {
    session
        .history()
        .entries()
        .filter(|e| e.changes.iter().any(|c| c.key.agent() == agent))
        .map(|e| e.meta.turn_id)
        .collect()
}

#[test]
fn a_childs_entries_carry_the_turn_id_root_was_on_when_they_were_written() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session.spawn_child(&root, ChildConfig::default()).expect("spawn");

    let turn_at_spawn = session.turn_id();
    session.step(user_input_for(&child, "第一轮"));

    let turns = turn_ids_touching(&session, &child);
    assert!(!turns.is_empty(), "spawn + 子的写入总该留下点什么");
    assert!(turns.iter().all(|t| *t == turn_at_spawn), "子的 entry 该继承 root 当时那一轮的 turn_id");
}

/// root 开新一轮之后，子在新一轮里写的 entry 该盖新的 `turn_id`——子自己的
/// 转移状态（Thinking -> Done）跟 root 在哪一轮完全无关，但落到日志上的记账
/// 归属永远跟着 root 当时的那个号走。
#[test]
fn a_new_root_turn_changes_the_turn_id_the_childs_later_entries_carry() {
    let mut session = new_session();
    let root = session.agent().clone();
    let child = session.spawn_child(&root, ChildConfig::default()).expect("spawn");
    session.step(user_input_for(&child, "开始思考")); // 子: Idle -> Thinking，落在 turn_one
    let turn_one = session.turn_id();

    session.begin_turn(); // 只动 root 自己的三个槽位，子的 Thinking 状态不受影响
    let turn_two = session.turn_id();
    assert_ne!(turn_one, turn_two, "begin_turn 该铸一个新号");

    // 子继续走自己的转移表（Thinking -> Done），这条 entry 该盖 root 此刻的
    // 新 turn_id，不是子出生/开始思考那一轮。
    session.step(provider_done_end_turn_for(&child, session.epoch(), "答案"));

    let touched = turn_ids_touching(&session, &child);
    assert!(touched.contains(&turn_one), "子出生 + 开始思考那批 entry 盖的是 turn_one");
    assert!(touched.contains(&turn_two), "子在 turn_two 里写的新 entry 该盖 turn_two，不是沿用 turn_one");
}
