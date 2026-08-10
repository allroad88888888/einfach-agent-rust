//! 101 验收：清一个这个 agent 历史里不存在的 `ToolCallId`——进 `unknown`，
//! 不进 `newly_cleared`，不 panic，也不悄悄当成功接受（101「定死的接口」注释：
//! 「静默接受会让 102 的 bug 藏在一个永远不生效的 id 里」）。

use agent_core::{AgentId, ToolCallId};

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

#[test]
fn clearing_an_unknown_id_is_recorded_as_unknown_and_does_not_panic() {
    let (mut session, _ids) = session_with_n_tool_calls(3);
    let root = AgentId::root();
    let ghost = ToolCallId::new("call_does_not_exist");

    let outcome = session.clear_tool_results(&root, [ghost.clone()]);

    assert!(
        outcome.newly_cleared.is_empty(),
        "不存在的 id 不该进 newly_cleared"
    );
    assert!(outcome.already_cleared.is_empty());
    assert_eq!(outcome.unknown, vec![ghost.clone()]);

    assert!(
        !session.send_plan_of(&root).cleared().contains(&ghost),
        "不存在的 id 不该真的被写进已清列表——那会让它成为一个永远不生效的死条目"
    );
}

/// 混合一个真实存在、一个不存在的 id：各自独立记账，互不影响。
#[test]
fn a_real_id_and_an_unknown_id_in_the_same_call_are_bucketed_independently() {
    let (mut session, ids) = session_with_n_tool_calls(3);
    let root = AgentId::root();
    let real = ids[0].clone();
    let ghost = ToolCallId::new("call_does_not_exist");

    let outcome = session.clear_tool_results(&root, [real.clone(), ghost.clone()]);

    assert_eq!(outcome.newly_cleared, vec![real.clone()]);
    assert!(outcome.already_cleared.is_empty());
    assert_eq!(outcome.unknown, vec![ghost]);
    assert_eq!(session.send_plan_of(&root).cleared().to_vec(), vec![real]);
}
