//! 101 额外验收三：先清 A 再清 B，已清列表顺序是 `[A, B]`，不是 `[B, A]`——
//! 首次加入顺序进 prompt（099 的 `cleared()` 契约），顺序变了序列化就跟着变
//! （红线 11：会进 prompt 的东西必须逐字节确定）。

use agent_core::AgentId;

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

#[test]
fn clearing_a_then_b_keeps_that_order_in_the_cleared_list() {
    let (mut session, ids) = session_with_n_tool_calls(2);
    let root = AgentId::root();
    let a = ids[0].clone();
    let b = ids[1].clone();

    let out_a = session.clear_tool_results(&root, [a.clone()]);
    assert_eq!(out_a.newly_cleared, vec![a.clone()]);

    let out_b = session.clear_tool_results(&root, [b.clone()]);
    assert_eq!(out_b.newly_cleared, vec![b.clone()]);

    assert_eq!(
        session.send_plan_of(&root).cleared().to_vec(),
        vec![a.clone(), b.clone()],
        "先清的 A 该排在先加入的位置，已清列表是 [A, B]"
    );
    assert_ne!(
        session.send_plan_of(&root).cleared().to_vec(),
        vec![b, a],
        "顺序翻过来就是不同的值——它决定了 SendPlan 序列化之后的字节"
    );
}

/// 反过来做一遍（先 B 后 A），排除「巧合按 id 字典序排列」这种假阳性——顺序
/// 该跟着调用顺序走，不是任何形式的排序。
#[test]
fn clearing_b_then_a_produces_the_reverse_order() {
    let (mut session, ids) = session_with_n_tool_calls(2);
    let root = AgentId::root();
    let a = ids[0].clone();
    let b = ids[1].clone();

    let _ = session.clear_tool_results(&root, [b.clone()]);
    let _ = session.clear_tool_results(&root, [a.clone()]);

    assert_eq!(
        session.send_plan_of(&root).cleared().to_vec(),
        vec![b, a],
        "这次是先清 B 后清 A，已清列表该是 [B, A]——不是按 id 排序"
    );
}
