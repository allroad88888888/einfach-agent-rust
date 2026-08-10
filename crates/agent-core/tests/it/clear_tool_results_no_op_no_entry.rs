//! 101 额外验收二：`newly_cleared` 为空时（入参全落进 `already_cleared` 和/或
//! `unknown`）——一条 entry 都不产生，undo 栈长度不变，`SendPlan` 本身也不变。

use agent_core::{AgentId, ToolCallId};

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

#[test]
fn all_already_cleared_input_leaves_no_new_entry() {
    let (mut session, ids) = session_with_n_tool_calls(3);
    let root = AgentId::root();

    let _ = session.clear_tool_results(&root, [ids[0].clone()]);
    let history_len = session.history_len();
    let plan = session.send_plan_of(&root);

    // 再清同一个 id：全落进 already_cleared，newly_cleared 是空的。
    let outcome = session.clear_tool_results(&root, [ids[0].clone()]);
    assert!(outcome.newly_cleared.is_empty());
    assert_eq!(outcome.already_cleared, vec![ids[0].clone()]);

    assert_eq!(
        session.history_len(),
        history_len,
        "newly_cleared 为空，不该多一条 entry"
    );
    assert_eq!(session.send_plan_of(&root), plan, "SendPlan 本身也不该变");
}

#[test]
fn all_unknown_input_leaves_no_new_entry() {
    let (mut session, _ids) = session_with_n_tool_calls(2);
    let root = AgentId::root();
    let history_len = session.history_len();
    let plan = session.send_plan_of(&root);

    let outcome = session.clear_tool_results(
        &root,
        [ToolCallId::new("nope_1"), ToolCallId::new("nope_2")],
    );
    assert!(outcome.newly_cleared.is_empty());
    assert!(outcome.already_cleared.is_empty());
    assert_eq!(outcome.unknown.len(), 2);

    assert_eq!(session.history_len(), history_len);
    assert_eq!(session.send_plan_of(&root), plan);
}

/// 混合桶：一部分已清、一部分未知，凑起来 newly_cleared 依然是空的——同样不落
/// entry。跟前两条分开测是因为「空」可以由任意组合凑出来，两种成分单独测过之后
/// 还得测「混在一起也一样」。
#[test]
fn a_mix_of_already_cleared_and_unknown_still_leaves_no_new_entry() {
    let (mut session, ids) = session_with_n_tool_calls(3);
    let root = AgentId::root();

    let _ = session.clear_tool_results(&root, [ids[0].clone()]);
    let history_len = session.history_len();
    let plan = session.send_plan_of(&root);

    let ghost = ToolCallId::new("call_ghost");
    let outcome = session.clear_tool_results(&root, [ids[0].clone(), ghost.clone()]);
    assert!(outcome.newly_cleared.is_empty());
    assert_eq!(outcome.already_cleared, vec![ids[0].clone()]);
    assert_eq!(outcome.unknown, vec![ghost]);

    assert_eq!(session.history_len(), history_len);
    assert_eq!(session.send_plan_of(&root), plan);
}
