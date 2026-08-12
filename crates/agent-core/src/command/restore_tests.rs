//! [`super::restore`]（`Session::restore`）的白盒单测：epoch/turn_id 取值、
//! redo 尾不写回、schema 演进的未知键，以及 160 加的「恢复带回非默认
//! `AgentLimits`」。拆自 `restore.rs`（它已经 303 行、红线 9 告警中，160 还要
//! 往里加东西），跟 `spawn.rs`/`spawn_tests.rs` 同一个拆分手法。

use super::*;
use crate::command::meta::EntryMeta;
use crate::engine::state::TurnStatus;
use crate::graph::Slot;
use crate::{ChildConfig, SpawnRefused};
use agent_store::Change;

fn agent() -> AgentId {
    AgentId::root()
}

fn meta(turn_id: u64, epoch: u64, label: &'static str) -> EntryMeta {
    EntryMeta {
        turn_id,
        epoch: Epoch(epoch),
        label,
        barrier: false,
    }
}

fn status_change(prev: TurnStatus, next: TurnStatus) -> Change<AtomKey, AgentValue> {
    Change {
        key: AtomKey::Agent(agent(), Slot::Status),
        prev: AgentValue::Status(prev),
        next: AgentValue::Status(next),
    }
}

/// 没有快照、entries 全部生效（cursor == len）：等价于「从头整份重放」。
#[test]
fn no_snapshot_replays_every_entry_up_to_cursor() {
    let entries = vec![AgentEntry {
        seq: 0,
        meta: meta(1, 0, "user_input"),
        changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
    }];
    let mut unknown = Vec::new();
    let session = Session::restore(
        agent(),
        None,
        entries,
        1,
        1,
        100,
        AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .unwrap();

    assert_eq!(session.status(), TurnStatus::Thinking);
    assert_eq!(session.turn_id(), 1);
    assert_eq!(session.epoch(), Epoch(1));
    assert!(unknown.is_empty());
}

/// 游标不在栈顶：`[cursor, len)` 是 redo 尾，**不写回** store，但仍然留在
/// `History` 里，`redo_turn` 应该能把它找回来。
#[test]
fn entries_past_the_cursor_are_not_replayed_but_stay_redoable() {
    let entries = vec![
        AgentEntry {
            seq: 0,
            meta: meta(1, 0, "user_input"),
            changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
        },
        AgentEntry {
            seq: 1,
            meta: meta(1, 0, "cancel"),
            changes: vec![status_change(
                TurnStatus::Thinking,
                TurnStatus::Failed(crate::engine::state::Failure::Cancelled),
            )],
        },
    ];
    let mut unknown = Vec::new();
    let mut session = Session::restore(
        agent(),
        None,
        entries,
        1,
        2,
        100,
        AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .unwrap();

    // 只应用了第一条：状态是 Thinking，不是 Cancelled。
    assert_eq!(session.status(), TurnStatus::Thinking);
    assert_eq!(session.cursor(), 1);
    assert_eq!(session.history_len(), 2);

    // redo 能把第二条找回来——它没有丢，只是没被应用。
    let report = session.redo_turn();
    assert!(matches!(
        report,
        crate::command::UndoReport::Applied { entries: 1, .. }
    ));
    assert_eq!(
        session.status(),
        TurnStatus::Failed(crate::engine::state::Failure::Cancelled)
    );
}

/// 快照 + 之后的日志：快照灌回 primitive，日志接着把状态推到快照点之后。
#[test]
fn a_snapshot_seeds_primitives_then_entries_advance_past_it() {
    let snapshot = vec![(
        AtomKey::Agent(agent(), Slot::Status),
        AgentValue::Status(TurnStatus::Thinking),
    )];
    let entries = vec![AgentEntry {
        seq: 5,
        meta: meta(3, 2, "provider_failed"),
        changes: vec![status_change(
            TurnStatus::Thinking,
            TurnStatus::Failed(crate::engine::state::Failure::Provider(
                crate::seam::ErrorClass::Unknown,
            )),
        )],
    }];
    let session = Session::restore(
        agent(),
        Some(snapshot),
        entries,
        1,
        6,
        100,
        AgentLimits::default(),
        &mut |_| panic!("不该有不认识的键"),
    )
    .unwrap();

    assert_eq!(session.turn_id(), 3);
    assert_eq!(session.epoch(), Epoch(3));
    assert!(matches!(session.status(), TurnStatus::Failed(_)));
}

/// 快照里有一个这一版 schema 已经不认识的键——`on_unknown_key` 收到，不 panic，
/// 其余照常灌回。
#[test]
fn an_unknown_snapshot_key_is_reported_not_silently_dropped() {
    let dropped_key = AtomKey::ToolCall(
        agent(),
        crate::ids::ToolCallId::new("gone"),
        crate::graph::ToolCallSlot::Result,
    );
    let snapshot = vec![
        (
            AtomKey::Agent(agent(), Slot::Status),
            AgentValue::Status(TurnStatus::Idle),
        ),
        (
            dropped_key.clone(),
            AgentValue::Text(std::sync::Arc::from("旧版本的东西")),
        ),
    ];
    let mut unknown = Vec::new();
    let session = Session::restore(
        agent(),
        Some(snapshot),
        Vec::new(),
        0,
        0,
        100,
        AgentLimits::default(),
        &mut |k| unknown.push(k.clone()),
    )
    .unwrap();

    assert_eq!(unknown, vec![dropped_key]);
    assert_eq!(session.status(), TurnStatus::Idle);
    assert_eq!(session.turn_id(), 1); // 没有 entry，退回起点
    assert_eq!(session.epoch(), Epoch::START);
}

/// 破坏 `History::from_parts` 不变量的落盘件原样拒绝，不硬凑。
#[test]
fn invalid_persisted_history_is_rejected() {
    let entries = vec![AgentEntry {
        seq: 0,
        meta: meta(1, 0, "user_input"),
        changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
    }];
    let Err(err) = Session::restore(
        agent(),
        None,
        entries,
        5, /* 越界 */
        1,
        100,
        AgentLimits::default(),
        &mut |_| {},
    ) else {
        panic!("越界游标该被拒绝");
    };
    assert_eq!(err, InvalidHistory::CursorOutOfRange);
}

// ---- 160：恢复带回宿主那一组 `AgentLimits` ----
//
// 这三条钉的是同一件事的三个面：值带回来了、闸真的按它拦人、它没有偷偷混进
// 落盘产物。`limits` 是**配置不是状态**（`Session` 字段表的既有拍板），所以
// 它的正确性完全靠「宿主把自己那一份再说一遍」这条通道——通道断了不会报错，
// 只会让恢复出来的会话悄悄退回默认档。

fn tight() -> AgentLimits {
    AgentLimits {
        max_depth: 2,
        max_children: 2,
    }
}

/// 主断言：传进去的非默认上限，恢复出来的会话原样拿着。
///
/// 160 之前这条必红——`restore` 把 `limits` 硬写成 `AgentLimits::default()`，
/// 入参根本不存在。
#[test]
fn a_restored_session_carries_the_limits_the_host_passed_in() {
    let session = Session::restore(agent(), None, Vec::new(), 0, 0, 100, tight(), &mut |_| {})
        .expect("恢复该成功");

    assert_eq!(session.agent_limits(), tight());
    assert_ne!(
        session.agent_limits(),
        AgentLimits::default(),
        "拿到的必须是宿主给的那组，不是默认档——两者相等就说明这条通道是断的"
    );
}

/// 光带回值不够：真正拦人的那道闸（`spawn_child`）也得按这组数拦。
///
/// 描述里的数字和拦人的数字必须是同一组，这是 `ToolTable::with_spawn` 与
/// `ToolTableSpec` 反复记着的耦合；恢复路径不是例外。
#[test]
fn the_gate_on_a_restored_session_uses_those_limits_not_the_defaults() {
    let mut session = Session::restore(agent(), None, Vec::new(), 0, 0, 100, tight(), &mut |_| {})
        .expect("恢复该成功");
    let root = agent();

    session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("第 1 个子该成功");
    session
        .spawn_child(&root, ChildConfig::default(), None)
        .expect("第 2 个子该成功");

    // 默认档是 8，这里必须在第 3 个就撞顶——撞的是宿主配的 2。
    match session.spawn_child(&root, ChildConfig::default(), None) {
        Err(SpawnRefused::TooManyChildren { live, max }) => {
            assert_eq!(max, 2, "撞的必须是宿主配的上限，不是 DEFAULT_MAX_CHILDREN");
            assert_eq!(live, 2);
        }
        other => panic!("第 3 个子该被 TooManyChildren 拒，实际：{other:?}"),
    }
}

/// `limits` 是配置不是状态：它不进原子图、不进日志。**换一组上限恢复，落盘
/// 产物一条 entry 都不该多**——它要是偷偷 journaled 了，红线 3/4 就被绕过去了
/// （而且 undo 会撞上「撤回一次上限变更」这个 `spawn.rs:91` 明确否决过的语义）。
#[test]
fn limits_do_not_leak_into_the_log() {
    let entries = vec![AgentEntry {
        seq: 0,
        meta: meta(1, 0, "user_input"),
        changes: vec![status_change(TurnStatus::Idle, TurnStatus::Thinking)],
    }];
    let with_default = Session::restore(
        agent(),
        None,
        entries.clone(),
        1,
        1,
        100,
        AgentLimits::default(),
        &mut |_| {},
    )
    .expect("恢复该成功");
    let with_tight = Session::restore(agent(), None, entries, 1, 1, 100, tight(), &mut |_| {})
        .expect("恢复该成功");

    assert_eq!(
        with_default.history_len(),
        with_tight.history_len(),
        "上限不同不该让日志长出（或少掉）任何一条 entry"
    );
    assert_eq!(with_default.status(), with_tight.status());
}
