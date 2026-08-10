//! 101 额外验收一：`ClearOutcome` 三个桶（`newly_cleared` / `already_cleared` /
//! `unknown`）互不相交，并集等于入参去重后的集合——包括入参本身带重复项、
//! 以及入参同时踩中「已清」「未知」两种既有状态的情况。

use std::collections::BTreeSet;

use agent_core::{AgentId, ToolCallId};

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

#[test]
fn outcome_buckets_are_disjoint_and_union_equals_deduped_input() {
    let (mut session, ids) = session_with_n_tool_calls(4);
    let root = AgentId::root();

    // 先把 ids[0] 清掉，让它在下一次调用里落进 already_cleared。
    let pre = session.clear_tool_results(&root, [ids[0].clone()]);
    assert_eq!(pre.newly_cleared, vec![ids[0].clone()]);

    let ghost = ToolCallId::new("call_ghost");
    // 入参：已清的 ids[0]、全新的 ids[1]（重复出现两次）、不存在的 ghost。
    let input = vec![
        ids[0].clone(),
        ids[1].clone(),
        ghost.clone(),
        ids[1].clone(),
    ];

    let outcome = session.clear_tool_results(&root, input.clone());

    let newly: BTreeSet<_> = outcome.newly_cleared.iter().cloned().collect();
    let already: BTreeSet<_> = outcome.already_cleared.iter().cloned().collect();
    let unknown: BTreeSet<_> = outcome.unknown.iter().cloned().collect();

    assert!(newly.is_disjoint(&already), "newly 与 already 不该有交集");
    assert!(newly.is_disjoint(&unknown), "newly 与 unknown 不该有交集");
    assert!(already.is_disjoint(&unknown), "already 与 unknown 不该有交集");

    let mut union: BTreeSet<_> = newly.union(&already).cloned().collect();
    union.extend(unknown.iter().cloned());
    let deduped_input: BTreeSet<_> = input.iter().cloned().collect();
    assert_eq!(
        union, deduped_input,
        "三桶并集该等于入参去重后的集合，一个不多一个不少"
    );

    // 具体归属也对得上：ids[0] 已清、ids[1] 新清（去重成一条）、ghost 未知。
    assert_eq!(outcome.newly_cleared, vec![ids[1].clone()]);
    assert_eq!(outcome.already_cleared, vec![ids[0].clone()]);
    assert_eq!(outcome.unknown, vec![ghost]);
}
