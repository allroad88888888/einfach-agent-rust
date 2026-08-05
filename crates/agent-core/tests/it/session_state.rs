//! 026 等价重写自 `turn_state.rs` 与 `tools_converged.rs` 里那些「状态本身长什么样」
//! 的断言：开局默认值、铸号、可序列化。
//!
//! 加了一条 M1 没有也不可能有的：**「完整状态 = 所有 primitive」**。M1 的 `TurnState`
//! 是一个平结构，「完整状态」只能靠人对着字段清单点名；这里它是
//! `Session::primitives()` 的返回值，点名的是构图函数。

use crate::support;
use agent_core::{AgentValue, AtomKey, Slot, TurnStatus};

use crate::support::session::new_session;

/// 开局：`Idle`、空历史、空槽、[`Epoch::START`]、无前缀镜像、下一个消息号是 1、
/// 轮数与重试计数清零、上限取默认值——逐条对应 M1 的
/// `new_turn_state_starts_with_zero_usage_and_default_caps`。
#[test]
fn a_fresh_session_starts_at_the_documented_defaults() {
    let s = new_session();

    assert_eq!(s.status(), TurnStatus::Idle);
    assert!(s.messages().is_empty());
    assert!(s.tool_slots().is_empty());
    assert!(s.tools_converged(), "零个槽位算收敛（没有东西要等）");
    assert_eq!(s.epoch(), agent_core::Epoch::START);
    assert!(s.prev_prefix().is_none());
    assert_eq!(s.next_message_id().0, 1);
    assert_eq!(s.turns_used(), 0);
    assert_eq!(s.max_turns(), 32);
    assert_eq!(s.retries_used(), 0);
    assert_eq!(s.max_retries(), 2);
    assert_eq!(s.turn_id(), 1);
    assert_eq!(s.history_len(), 0, "建图不是状态变更，不该留下 entry");
}

/// **完整状态 = 所有 primitive**：一个新会话的图上恰好是 `Slot::ALL` 那九个槽位，
/// 每个持它自己的默认值。
///
/// 这条测的是构图函数与槽位表没有分家。分家的症状是快照少一项、恢复时那一项落
/// 默认值——碰巧默认值就是它当时的值，于是永远不报错，直到某天默认值改了。
#[test]
fn the_complete_state_is_exactly_the_source_slot_table() {
    let s = new_session();
    let primitives = s.primitives();

    let agent = support::agent();
    let expected: Vec<(AtomKey, AgentValue)> = {
        let mut v: Vec<_> = Slot::ALL
            .iter()
            .map(|slot| (AtomKey::Agent(agent.clone(), *slot), slot.default_value()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    };
    assert_eq!(primitives, expected);
    // 条数写死一份（028 是 10：026 的九个 + `Slot::ToolsAllowed`；039 加
    // `Slot::SkillsActive` → 11；073 加 `Slot::HostTools` → 12；064 加
    // `Slot::HostSkills` → 13；076 加 `Slot::DisabledBuiltins` → 14）——上面那句比的是「和 `Slot::ALL` 一致」，
    // 这句比的是「`Slot::ALL` 本身没被顺手加过东西」。
    assert_eq!(primitives.len(), 14);

    // derived 一个都不在里面——它们的键是另一张表。
    assert!(
        primitives
            .iter()
            .all(|(key, _)| matches!(key, AtomKey::Agent(_, _)))
    );
}

/// 快照可 serde 往返（红线 3：primitive 的值必须**全部**可序列化）。这是 010 的
/// `Snapshot` 形状：`Vec<(AtomKey, Value)>`，键是逻辑键（红线 4）。
#[test]
fn the_snapshot_of_a_real_conversation_survives_a_serde_roundtrip() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("读一下 a.txt"));
    let _ = s.step(support::provider_done_tool_use(
        s.epoch(),
        &[("call_1", "srv:fs/read")],
    ));

    let snapshot = s.primitives();
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: Vec<(AtomKey, AgentValue)> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, snapshot);
    // 键是逻辑键，不是自增句柄——落盘的东西里不该出现「第几个被创建的」这种信息。
    assert!(json.contains("Messages"));
    assert!(json.contains("root"));
}

/// 铸号：从 1 起严格递增，user 和 assistant 消息共用同一个号池。
#[test]
fn message_ids_start_at_one_and_increment_across_roles() {
    let mut s = new_session();
    assert_eq!(s.next_message_id().0, 1);

    let _ = s.step(support::user_input_event("一"));
    assert_eq!(s.messages().back().unwrap().id.0, 1);
    assert_eq!(s.next_message_id().0, 2);

    let _ = s.step(support::provider_done_end_turn(s.epoch(), "二"));
    assert_eq!(s.messages().back().unwrap().id.0, 2);
    assert_eq!(s.next_message_id().0, 3);
}

/// 公开读口给出的是**值的克隆**，改它改不到会话——`Session` 不暴露 store，
/// 也就没有「拿到引用顺手改一下」这条绕过 command 层的路（红线 2）。
#[test]
fn the_read_surface_hands_out_clones_not_handles() {
    let mut s = new_session();
    let _ = s.step(support::user_input_event("一"));

    let mut mine = s.messages();
    mine.clear();
    assert_eq!(s.messages().len(), 1, "改克隆件改不到会话状态");

    let slots = s.tool_slots();
    drop(slots);
    assert_eq!(s.messages().len(), 1);
}
