//! 101 验收：幂等——重复清同一批 `ToolCallId`，已清列表不出现重复项，`prev`
//! 也不因此变化，且第二次调用不产生新的 undo entry（`newly_cleared` 是空的时候
//! 不写、不进 undo log，是「定死的接口」文档注释里明写的一条）。

use std::collections::BTreeSet;

use agent_core::AgentId;

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

#[test]
fn clearing_the_same_ids_twice_is_idempotent_and_leaves_no_second_entry() {
    let (mut session, ids) = session_with_n_tool_calls(5);
    let root = AgentId::root();
    let subset = vec![ids[0].clone(), ids[1].clone()];

    let first = session.clear_tool_results(&root, subset.clone());
    assert_eq!(first.newly_cleared, subset);
    assert!(first.already_cleared.is_empty());
    assert!(first.unknown.is_empty());

    let history_len_after_first = session.history_len();
    let plan_after_first = session.send_plan_of(&root);

    let second = session.clear_tool_results(&root, subset.clone());
    assert!(
        second.newly_cleared.is_empty(),
        "同一批 id 第二次清，一个都不该算新加入"
    );
    assert_eq!(second.already_cleared, subset);
    assert!(second.unknown.is_empty());

    assert_eq!(
        session.history_len(),
        history_len_after_first,
        "newly_cleared 为空，不该多出一条 entry"
    );
    assert_eq!(
        session.send_plan_of(&root),
        plan_after_first,
        "重复清不该改变 SendPlan 本身（包括它的 prev 链——没有新 entry 就没有新 prev）"
    );

    // 已清列表本身没有重复项。
    let cleared = session.send_plan_of(&root).cleared().to_vec();
    let unique: BTreeSet<_> = cleared.iter().cloned().collect();
    assert_eq!(cleared.len(), unique.len(), "已清列表不该出现重复项");
    assert_eq!(cleared, subset);
}

/// 幂等要顶得住反复调用，不只是调用两次——多清几轮同一批 id，状态该收敛在
/// 第一次清完之后就不再变化。
#[test]
fn clearing_repeatedly_converges_after_the_first_call() {
    let (mut session, ids) = session_with_n_tool_calls(3);
    let root = AgentId::root();
    let target = ids[0].clone();

    let _ = session.clear_tool_results(&root, [target.clone()]);
    let plan_after_first = session.send_plan_of(&root);
    let history_len_after_first = session.history_len();

    for _ in 0..10 {
        let outcome = session.clear_tool_results(&root, [target.clone()]);
        assert!(outcome.newly_cleared.is_empty());
        assert_eq!(outcome.already_cleared, vec![target.clone()]);
    }

    assert_eq!(session.send_plan_of(&root), plan_after_first);
    assert_eq!(session.history_len(), history_len_after_first);
}
