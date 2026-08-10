//! 102 验收：触发之后，保护区之外的工具结果**一个不剩**、保护区之内的**一个
//! 没动**——以及「当前轮」作为保护区里 N=1 的特例（096 §四第二条）。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

const TURNS: usize = 10;
const PROTECT: usize = 3;

/// 一次全清：保护区之外的 7 轮一个不剩地全部出现，保护区之内的 3 轮一个没动
/// ——用集合精确相等而不是子集包含来判定，防止「只清了一部分」蒙混过关。
#[test]
fn reachable_results_are_returned_completely_and_protected_ones_are_untouched() {
    let (history, ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);

    let reachable = &ids[..TURNS - PROTECT];
    let protected = &ids[TURNS - PROTECT..];

    assert_eq!(
        cleared, reachable,
        "保护区之外的工具结果必须一个不剩地全部返回"
    );
    for id in protected {
        assert!(
            !cleared.contains(id),
            "{id:?} 属于最近 {PROTECT} 轮，不该出现在清单里"
        );
    }
}

/// `protect_recent_turns = 1`：只保护「当前这一轮」，验证保护区的最小取值刚好
/// 对应 096 §四第二条「当前轮的工具返回不能清」——它不是一条独立机制，是
/// N >= 1 时保护区天然覆盖到的那一轮。
#[test]
fn protect_recent_turns_one_only_protects_the_current_turn() {
    const N: usize = 5;
    let (history, ids) = history_with_turns(N);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 1);

    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);

    assert_eq!(cleared, ids[..N - 1].to_vec());
    assert!(
        !cleared.contains(&ids[N - 1]),
        "当前轮（最后一轮）不该被清"
    );
}
