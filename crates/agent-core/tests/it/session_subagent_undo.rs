//! 028：跨 agent 的 undo —— 「都在一个 store，undo 回滚整个」那句口号的实检。
//!
//! 验收对应：
//! - 「undo 一轮连带子树」：root 轮内 spawn 子 + 子写状态 → `undo_turn` → 子树
//!   primitive 全回退
//! - 「despawn → undo → 子树值完整重建」（019 链路的跨 agent 版）
//!
//! # 本 issue 裁决的 undo-子树语义
//!
//! 轮内 spawn 的子在 undo 之后：**atom 留在图上，值回 spawn 前的默认值**
//! （`ToolsAllowed` 回 `Null`，也就是不在活名单上）。**不是**连 atom 一起 despawn。
//! 理由与代价写在 `command/tree.rs` 与 `command/despawn.rs` 的模块文档里，
//! 这个文件是它的可执行形式：`redo_turn` 之后子树必须**完整**回来。

mod support;

use std::sync::Arc;

use agent_core::{
    AgentId, AgentValue, AtomKey, ChildConfig, Session, Slot, TurnStatus, UndoReport,
};
use support::user_input_event;
use support::user_input_for;

fn subtree_of(s: &Session, agent: &AgentId) -> Vec<(AtomKey, AgentValue)> {
    s.primitives()
        .into_iter()
        .filter(|(k, _)| k.agent() == agent)
        .collect()
}

fn cfg() -> ChildConfig {
    ChildConfig {
        tools_allowed: vec![Arc::from("srv:fs/read")],
    }
}

/// 一轮里：root 说话 → spawn 一个子 → 子干活。`undo_turn` 一次全退。
#[test]
fn one_undo_turn_takes_the_whole_subtree_with_it() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();

    let _ = s.step(user_input_event("帮我分解一下这个任务"));
    let child = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "子任务：读文件"));

    assert!(s.is_live(&child));
    assert_eq!(
        s.read_descendant(&root, &child, Slot::Status)
            .unwrap()
            .as_status()
            .unwrap(),
        &TurnStatus::Thinking
    );
    let live = subtree_of(&s, &child);

    // 三条 entry 全在同一个 root turn 里（决策 5：子 agent 继承 turn_id）。
    for entry in s.history().entries() {
        assert_eq!(entry.meta.turn_id, 1, "{:?}", entry.meta.label);
    }

    let report = s.undo_turn();
    assert!(matches!(report, UndoReport::Applied { turn_id: 1, .. }));

    // 子树的 primitive **全部**回到默认值——一个不差。
    let after = subtree_of(&s, &child);
    let expected: Vec<(AtomKey, AgentValue)> = Slot::ALL
        .iter()
        .map(|slot| (AtomKey::Agent(child.clone(), *slot), slot.default_value()))
        .collect();
    let mut expected_sorted = expected;
    expected_sorted.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(after, expected_sorted);

    // atom 还在图上（019：applier 只写值不毁 atom），但 agent 不在活名单上。
    assert_eq!(after.len(), Slot::ALL.len(), "atom 一个都没被 undo 毁掉");
    assert!(!s.is_live(&child), "spawn 被撤了，它就不在活名单上");
    assert_eq!(s.children_of(&root), Vec::<AgentId>::new());
    assert_ne!(after, live);
}

/// 撤了要能原样回来——**这条是上面那个语义裁决的检验点**：
/// 「值回默认值」之所以自洽，是因为 redo 只需把 `next` 灌回去。
#[test]
fn redo_brings_the_whole_subtree_back() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();

    let _ = s.step(user_input_event("分解"));
    let child = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "子任务"));
    let live = subtree_of(&s, &child);

    let _ = s.undo_turn();
    let _ = s.redo_turn();

    assert_eq!(subtree_of(&s, &child), live, "redo 之后子树逐值相同");
    assert!(s.is_live(&child));
    assert_eq!(s.children_of(&root), vec![child]);
}

/// root 自己的状态和子的状态是**各自的槽位**：撤一轮两边一起退，
/// 不会出现「消息历史回滚了但子 agent 还在跑」。
#[test]
fn the_parent_and_the_child_rewind_together() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();

    let _ = s.step(user_input_event("一"));
    let child = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "二"));

    assert_eq!(s.messages().len(), 1);
    let _ = s.undo_turn();

    assert_eq!(s.messages().len(), 0, "root 的消息历史回退了");
    assert_eq!(s.status(), TurnStatus::Idle);
    assert!(!s.is_live(&child), "子 agent 跟着一起退");
}

/// **despawn → undo → 子树值完整重建**（019 的链路第一次跨 agent 跑通）。
///
/// 逐出**不产生 `Change`**，所以能回来完全靠 teardown 那条 command 把活值记成
/// `prev`；重建靠 applier 的 `resolve` 是 get-or-create。两者缺一，undo 拿回的
/// 就是默认值——链通、值错、不报错。
#[test]
fn undoing_a_despawn_rebuilds_the_subtree_with_its_live_values() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();

    let child = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "干活"));
    let grandchild = s.spawn_child(&child, ChildConfig::default()).unwrap();
    let _ = s.step(user_input_for(&grandchild, "更细的活"));

    let child_live = subtree_of(&s, &child);
    let grand_live = subtree_of(&s, &grandchild);
    assert_eq!(child_live.len(), Slot::ALL.len());

    s.begin_turn(); // 让 despawn 落在下一轮，undo_turn 只退它
    let report = s.despawn_child(&child).unwrap();
    assert_eq!(
        report.agents,
        vec![grandchild.clone(), child.clone()],
        "自叶向根"
    );

    // 逐出真的发生了：每个 agent 只剩一个墓碑槽位。
    assert_eq!(subtree_of(&s, &child).len(), 1);
    assert_eq!(subtree_of(&s, &grandchild).len(), 1);
    assert!(!s.is_live(&child) && !s.is_live(&grandchild));

    let _ = s.undo_turn();

    assert_eq!(subtree_of(&s, &child), child_live, "子的槽位逐值回来");
    assert_eq!(subtree_of(&s, &grandchild), grand_live, "孙的槽位逐值回来");
    assert!(s.is_live(&child) && s.is_live(&grandchild));
    assert_eq!(
        s.read_descendant(&root, &child, Slot::Status)
            .unwrap()
            .as_status()
            .unwrap(),
        &TurnStatus::Thinking,
        "重建出来的 atom 接回了图，读得到活值"
    );
}

/// 重建之后那些 atom 是**活的**，不是一次性灌完就断线的死值：再写一次，
/// 子 agent 的转移照常发生。
#[test]
fn a_rebuilt_child_keeps_working() {
    let mut s = Session::new(AgentId::root());
    let root = AgentId::root();
    let child = s.spawn_child(&root, cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "干活"));

    s.begin_turn();
    let _ = s.despawn_child(&child).unwrap();
    let _ = s.undo_turn();

    let effects = s.step(support::provider_done_end_turn_for(
        &child,
        s.epoch(),
        "干完了",
    ));
    assert!(!effects.is_empty());
    assert_eq!(
        s.read_descendant(&root, &child, Slot::Status)
            .unwrap()
            .as_status()
            .unwrap(),
        &TurnStatus::Done { truncated: false }
    );
}
