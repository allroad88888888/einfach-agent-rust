//! 046：`Session::agent_tree()` 的独立验收测试——**只测规格，不看实现**。
//!
//! `agent_tree()` 目前是 `todo!()`（接口先钉死，实现与本测试并行），所以这个文件
//! 现在跑起来会 panic，这是预期状态：测试对着 `docs/issues/046-agent-tree.md` 的
//! 验收一节和 `crates/agent-core/src/observe.rs` 的 pub 签名/文档写，实现填好之后
//! 应当自然变绿，不需要回来改这个文件。
//!
//! 验收对应（`docs/issues/046-agent-tree.md` §「验收」）：
//! - 单 agent 会话：1 个节点，`parent = None`，`depth = 0`，`activity` 跟
//!   `status_of(root)` 对得上 —— [`new_session_has_a_single_idle_root_node`]。
//! - spawn 两个子 agent：3 个节点，两个子 `parent = root`、`depth = 1`，顺序稳定
//!   —— [`spawning_two_children_yields_three_nodes_in_stable_order`]。
//! - activity 映射（Idle/Thinking 两个稳的 + Working/Done/Failed 能造出来的）
//!   —— 见下面按状态命名的测试，全部经 [`assert_activity_matches_status`] 校验。
//! - **undo 一致**（本 issue 最关键的断言，红线 1/4 的实检）
//!   —— [`undo_of_the_spawn_turn_drops_the_child_from_the_tree`]，外加
//!   [`redo_after_undo_brings_the_child_back_into_the_tree`] 补一圈 undo/redo
//!   都得跟树对得上。
//! - `task` = 该 agent 的第一条 user 消息，且不随后续消息改变
//!   —— [`root_task_is_the_first_message_and_survives_a_second_turn`]、
//!   [`child_task_is_its_own_spawn_message_not_roots`]。
//!
//! 只驱动 `Session` 的公开 API（`step`/`spawn_child`/`begin_turn`/`undo_turn`/
//! `redo_turn`），事件构造复用 `support/mod.rs`——跟
//! `session_subagent_spawn.rs`/`session_subagent_undo.rs`/
//! `session_subagent_step_routing.rs` 是同一套打法，那几个文件也是本文件驱动
//! spawn/undo/多 agent 状态时对照的样板。

mod support;

use std::sync::Arc;

use agent_core::{
    AgentActivity, AgentId, AgentNode, AgentTree, ChildConfig, Event, Session, TurnStatus,
    UndoReport,
};
use support::{provider_done_end_turn, provider_done_end_turn_for, provider_done_tool_use_for, user_input_event, user_input_for};

fn root() -> AgentId {
    AgentId::root()
}

fn cfg() -> ChildConfig {
    ChildConfig { tools_allowed: vec![Arc::from("srv:fs/read")] }
}

fn find<'a>(tree: &'a AgentTree, id: &AgentId) -> &'a AgentNode {
    tree.nodes.iter().find(|n| &n.id == id).unwrap_or_else(|| {
        panic!("agent_tree() 里没有 {id:?}，节点里的 id 有：{:?}", tree.nodes.iter().map(|n| &n.id).collect::<Vec<_>>())
    })
}

/// `activity` 是 `status_of` 的呈现投影，不是独立状态——046 的验收原文「activity
/// 跟 `status_of` 对得上」在这里落成一条可复用的断言，每个驱动出新状态的测试都
/// 过一遍它，而不是各自 `matches!` 一遍容易漏掉分支。
fn assert_activity_matches_status(activity: &AgentActivity, status: &TurnStatus) {
    match status {
        TurnStatus::Idle => {
            assert_eq!(activity, &AgentActivity::Idle, "Idle 应映射成 AgentActivity::Idle");
        }
        TurnStatus::Thinking => {
            assert_eq!(activity, &AgentActivity::Thinking, "Thinking 应映射成 AgentActivity::Thinking");
        }
        TurnStatus::ToolsPending => {
            assert!(
                matches!(activity, AgentActivity::Working { .. }),
                "ToolsPending 应映射成 AgentActivity::Working{{..}}，实际 {activity:?}"
            );
        }
        TurnStatus::Done { truncated } => {
            assert_eq!(
                activity,
                &AgentActivity::Done { truncated: *truncated },
                "Done{{truncated}} 应原样带过来"
            );
        }
        TurnStatus::Failed(_) => {
            assert!(
                matches!(activity, AgentActivity::Failed { .. }),
                "Failed 应映射成 AgentActivity::Failed{{reason}}，实际 {activity:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 单 agent 新会话
// ---------------------------------------------------------------------------

#[test]
fn new_session_has_a_single_idle_root_node() {
    let s = Session::new(root());

    let tree = s.agent_tree();
    assert_eq!(tree.nodes.len(), 1, "新会话只有 root 一个节点");

    let node = &tree.nodes[0];
    assert_eq!(node.id, root());
    assert_eq!(node.parent, None, "root 没有父");
    assert_eq!(node.depth, 0, "root 深度为 0");
    assert_eq!(node.task, None, "还没有任何 user 消息");
    assert_activity_matches_status(&node.activity, &s.status());
    assert_eq!(node.activity, AgentActivity::Idle);
}

// ---------------------------------------------------------------------------
// spawn 两个子 agent：节点数、parent/depth、顺序稳定
// ---------------------------------------------------------------------------

#[test]
fn spawning_two_children_yields_three_nodes_in_stable_order() {
    let mut s = Session::new(root());
    let a1 = s.spawn_child(&root(), cfg()).unwrap();
    let a2 = s.spawn_child(&root(), cfg()).unwrap();

    let first = s.agent_tree();
    assert_eq!(first.nodes.len(), 3, "root + 两个子");

    let root_node = find(&first, &root());
    assert_eq!(root_node.parent, None);
    assert_eq!(root_node.depth, 0);

    for child in [&a1, &a2] {
        let node = find(&first, child);
        assert_eq!(node.parent, Some(root()), "子的 parent 是 root");
        assert_eq!(node.depth, 1, "子深度为 1");
    }

    // 顺序稳定：同一状态两次调用，节点的 id 序列逐个相同——树渲染不该抖。
    let second = s.agent_tree();
    let ids_first: Vec<AgentId> = first.nodes.iter().map(|n| n.id.clone()).collect();
    let ids_second: Vec<AgentId> = second.nodes.iter().map(|n| n.id.clone()).collect();
    assert_eq!(ids_first, ids_second, "两次调用 agent_tree() 的节点顺序必须逐个相同");

    // 具体顺序：root 在前、字典序（跟 `Session::live_agents()` 的裁决同一句话）。
    assert_eq!(ids_first, vec![root(), a1, a2]);
}

// ---------------------------------------------------------------------------
// activity 映射：Idle / Thinking（两个稳的）
// ---------------------------------------------------------------------------

#[test]
fn root_activity_is_thinking_right_after_the_first_message() {
    let mut s = Session::new(root());
    let _ = s.step(user_input_event("帮我读一下 a.txt"));
    assert_eq!(s.status(), TurnStatus::Thinking, "fixture 前提：驱动到了 Thinking");

    let tree = s.agent_tree();
    let node = find(&tree, &root());
    assert_activity_matches_status(&node.activity, &s.status());
    assert_eq!(node.activity, AgentActivity::Thinking);
}

#[test]
fn a_child_that_has_not_been_spoken_to_yet_stays_idle() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg()).unwrap();

    let tree = s.agent_tree();
    let node = find(&tree, &child);
    assert_eq!(node.task, None, "spawn 本身不落一条 user 消息");
    assert_activity_matches_status(&node.activity, &s.status_of(&child));
    assert_eq!(node.activity, AgentActivity::Idle);
}

// ---------------------------------------------------------------------------
// activity 映射：Working（ToolsPending）
// ---------------------------------------------------------------------------

#[test]
fn a_child_running_a_tool_shows_up_as_working() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg()).unwrap();

    let _ = s.step(user_input_for(&child, "子任务：读文件"));
    let _ = s.step(provider_done_tool_use_for(&child, s.epoch(), &[("call_1", "srv:fs/read")]));
    assert_eq!(s.status_of(&child), TurnStatus::ToolsPending, "fixture 前提");

    let tree = s.agent_tree();
    let node = find(&tree, &child);
    assert_activity_matches_status(&node.activity, &s.status_of(&child));
    let AgentActivity::Working { tools } = &node.activity else {
        panic!("期待 Working{{..}}，实际 {:?}", node.activity);
    };
    // 046 原文：工具名是「锦上添花」，一时推不出可以是空 Vec；「在忙」这个事实本身
    // 才是硬约束（上面那条 assert_activity_matches_status 已经钉死）。这里只在
    // 非空时校验内容对不对，不强求非空。
    if !tools.is_empty() {
        assert!(
            tools.iter().any(|t| &**t == "srv:fs/read"),
            "非空时至少应该包含在飞的工具名，实际 {tools:?}"
        );
    }

    // root 没被动过：agent_tree() 是纯读，不该把别的 agent 也带跑偏。
    let root_node = find(&tree, &root());
    assert_eq!(root_node.activity, AgentActivity::Idle);
}

// ---------------------------------------------------------------------------
// activity 映射：Done / Failed
// ---------------------------------------------------------------------------

#[test]
fn a_child_that_finishes_its_turn_shows_up_as_done() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg()).unwrap();

    let _ = s.step(user_input_for(&child, "子任务"));
    let _ = s.step(provider_done_end_turn_for(&child, s.epoch(), "干完了"));
    assert_eq!(s.status_of(&child), TurnStatus::Done { truncated: false }, "fixture 前提");

    let tree = s.agent_tree();
    let node = find(&tree, &child);
    assert_activity_matches_status(&node.activity, &s.status_of(&child));
    assert_eq!(node.activity, AgentActivity::Done { truncated: false });
}

#[test]
fn a_cancelled_child_shows_up_as_failed() {
    let mut s = Session::new(root());
    let child = s.spawn_child(&root(), cfg()).unwrap();

    // 016 验收原文「取消在任意状态下都生效」——从 Idle 直接 Cancel 就够造出 Failed，
    // 不需要先把它推进 Thinking/ToolsPending。
    let _ = s.step(Event::Cancel { agent: child.clone() });
    assert!(matches!(s.status_of(&child), TurnStatus::Failed(_)), "fixture 前提");

    let tree = s.agent_tree();
    let node = find(&tree, &child);
    assert_activity_matches_status(&node.activity, &s.status_of(&child));
    assert!(matches!(node.activity, AgentActivity::Failed { .. }));
}

// ---------------------------------------------------------------------------
// undo 一致 —— 本 issue 最关键的断言（红线 1/4 的实检）
// ---------------------------------------------------------------------------

/// spawn 子 agent 那一轮被 `undo_turn` 撤掉之后，`agent_tree()` **不再**含那个
/// 子 agent。这条断言把「派生读不纯」或「捕获了 `AtomId`」这两种红线 1/4 违规
/// 变成一个会失败的测试——树跟着状态回退必须是零专门代码的自动结果。
#[test]
fn undo_of_the_spawn_turn_drops_the_child_from_the_tree() {
    let mut s = Session::new(root());

    let _ = s.step(user_input_event("帮我分解一下这个任务"));
    let child = s.spawn_child(&root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "子任务：读文件"));

    let before = s.agent_tree();
    assert_eq!(before.nodes.len(), 2, "root + 子");
    assert!(before.nodes.iter().any(|n| n.id == child), "撤销之前子在树上");

    // 三条 entry（user_input / spawn_child / user_input）全在同一个 root turn 里
    // （028 的决策 5：子 agent 继承 turn_id），所以一次 `undo_turn` 该把三条一起退。
    for entry in s.history().entries() {
        assert_eq!(entry.meta.turn_id, 1, "{:?}", entry.meta.label);
    }

    let report = s.undo_turn();
    assert!(matches!(report, UndoReport::Applied { turn_id: 1, .. }));
    assert!(!s.is_live(&child), "spawn 被撤了，子不再活着");

    let after = s.agent_tree();
    assert_eq!(after.nodes.len(), 1, "被撤的子 agent 不再出现在 agent_tree() 里");
    assert_eq!(after.nodes[0].id, root());
    assert!(
        after.nodes.iter().all(|n| n.id != child),
        "红线 1/4 实检：undo 之后树里绝不能再含被撤的子 agent"
    );
}

/// 补一圈：undo 之后 redo，子 agent 要跟着**原样**回到树上——这是上面那条裁决
/// 自洽的检验点（`agent_tree()` 是纯派生读，redo 灌回 primitive 之后树自动重算）。
#[test]
fn redo_after_undo_brings_the_child_back_into_the_tree() {
    let mut s = Session::new(root());

    let _ = s.step(user_input_event("分解"));
    let child = s.spawn_child(&root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "子任务"));

    let _ = s.undo_turn();
    assert_eq!(s.agent_tree().nodes.len(), 1, "undo 之后只剩 root");

    let _ = s.redo_turn();
    assert!(s.is_live(&child));

    let tree = s.agent_tree();
    assert_eq!(tree.nodes.len(), 2, "redo 之后子 agent 回到树上");
    let node = find(&tree, &child);
    assert_eq!(node.parent, Some(root()));
    assert_eq!(node.depth, 1);
}

// ---------------------------------------------------------------------------
// task = 该 agent 的第一条 user 消息
// ---------------------------------------------------------------------------

#[test]
fn root_task_is_the_first_message_and_survives_a_second_turn() {
    let mut s = Session::new(root());
    let _ = s.step(user_input_event("第一句"));
    let _ = s.step(provider_done_end_turn(s.epoch(), "答完了"));
    assert_eq!(s.status(), TurnStatus::Done { truncated: false }, "fixture 前提：先走完一轮");

    let first_turn_tree = s.agent_tree();
    assert_eq!(find(&first_turn_tree, &root()).task, Some("第一句".to_string()));

    // 开第二轮、说第二句——task 仍然是第一句，不随后续消息改变。
    s.begin_turn();
    let _ = s.step(user_input_event("第二句"));

    let second_turn_tree = s.agent_tree();
    assert_eq!(
        find(&second_turn_tree, &root()).task,
        Some("第一句".to_string()),
        "task 是第一条 user 消息，不是最新一条"
    );
}

#[test]
fn child_task_is_its_own_spawn_message_not_roots() {
    let mut s = Session::new(root());
    let _ = s.step(user_input_event("root 自己的第一句"));
    let child = s.spawn_child(&root(), cfg()).unwrap();
    let _ = s.step(user_input_for(&child, "子任务：分析日志"));

    let tree = s.agent_tree();
    assert_eq!(find(&tree, &root()).task, Some("root 自己的第一句".to_string()));
    assert_eq!(
        find(&tree, &child).task,
        Some("子任务：分析日志".to_string()),
        "子 agent 的 task 是它自己第一条消息，不是 root 的"
    );
}
