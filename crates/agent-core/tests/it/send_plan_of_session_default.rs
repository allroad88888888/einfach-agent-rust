//! Issue 100 额外验收二 + `Session::replace_send_plan` 的命令层契约：
//! `send_plan_of` 对一个从没设过的 agent 返回 pristine 且不 panic；
//! `replace_send_plan` 真的改变了下一次 `send_plan_of` 的返回值，并且跟别的
//! primitive 一样走 command 层进 undo log——`undo_step` 能把它退回去。
//!
//! 只看公开签名（本 issue「定死的接口」那节给的两个方法），不看它们的实现体。

use agent_core::{AgentId, SendPlan, ToolCallId, UndoReport};

use crate::support::session::new_session;

/// 从没调用过 `replace_send_plan` 的 root：`send_plan_of` 返回 pristine
/// （`SendPlan::new()`），不是 panic、不是某个未初始化的占位值。
#[test]
fn send_plan_of_a_never_touched_root_is_pristine() {
    let session = new_session();
    let plan = session.send_plan_of(&AgentId::root());
    assert!(plan.is_pristine());
    assert_eq!(plan, SendPlan::new());
}

/// 一个语法上合法、但从没被 `spawn_child` 建过图的 `AgentId`：`send_plan_of`
/// 照样返回 pristine，不 panic——跟 `read.rs` 里其它 `*_of` 口子「键不在
/// family 里就落默认值」的既有约定一致（`messages_of`/`prev_prefix_of` 等
/// 都是这个形状，`send_plan_of` 没有理由更特殊）。
#[test]
fn send_plan_of_an_agent_that_was_never_spawned_does_not_panic() {
    let session = new_session();
    let ghost = AgentId::root().child(99);
    assert!(!session.is_live(&ghost), "这个 id 压根没被 spawn 过");

    let plan = session.send_plan_of(&ghost);
    assert!(plan.is_pristine(), "没建过图的 agent 也该拿到 pristine，而不是 panic");
}

/// `replace_send_plan` 真的换掉了状态：换完之后 `send_plan_of` 返回的是新值，
/// 不再是 pristine。
#[test]
fn replace_send_plan_changes_what_send_plan_of_returns() {
    let mut session = new_session();
    let root = AgentId::root();

    let mut plan = SendPlan::new();
    plan.clear_tool_results([ToolCallId::new("call_1")]);
    session.replace_send_plan(&root, plan.clone());

    let got = session.send_plan_of(&root);
    assert!(!got.is_pristine());
    assert_eq!(got, plan);
}

/// `replace_send_plan` 走 command 层进 undo log——`undo_step` 能把它精确退回
/// 换之前的那个值，跟 `clear_prev_prefix` 等既有 primitive setter 同一条约定
/// （红线 2：业务代码禁止直接 `store.set()`，这里从行为上反过来验证了这一点：
/// 如果它是绕过 undo log 的裸写，这条会红）。
#[test]
fn undo_step_after_replace_send_plan_restores_the_previous_plan() {
    let mut session = new_session();
    let root = AgentId::root();
    assert!(session.send_plan_of(&root).is_pristine());

    let mut plan = SendPlan::new();
    plan.clear_tool_results([ToolCallId::new("call_1"), ToolCallId::new("call_2")]);
    session.replace_send_plan(&root, plan.clone());
    assert_eq!(session.send_plan_of(&root), plan);

    let report = session.undo_step();
    assert!(
        matches!(report, UndoReport::Applied { entries: 1, .. }),
        "{report:?}"
    );
    assert!(
        session.send_plan_of(&root).is_pristine(),
        "撤销之后该退回换之前的 pristine 状态"
    );

    // redo 是它的反演：把刚撤销的那个值追回来。
    let redo = session.redo_step();
    assert!(matches!(redo, UndoReport::Applied { entries: 1, .. }));
    assert_eq!(session.send_plan_of(&root), plan);
}
