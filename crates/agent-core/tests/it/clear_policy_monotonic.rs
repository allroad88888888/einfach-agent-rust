//! 102 验收：单调——先清一批写进 `plan`，再算一次，已清的不回来。
//!
//! 两个用例都刻意让**第二次调用依然触发**（只是压力比第一次小），而不是让它
//! 干脆不触发：如果第二次根本没触发，返回空只是「触发线拦住了」，没有真的
//! 走到「排除 `plan.cleared()`」这条逻辑，测试就抓不住「已清的被错误地放回来」
//! 这类 bug。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

const PROTECT: usize = 3;

/// 第一次全清之后，历史和保护区都没变，第二次哪怕换一组「更宽松」（离触发线
/// 更近、余量更大）但依然过线的用量重算，也只能是空——可及区能清的上一次已经
/// 清完了，没有新东西冒出来。
#[test]
fn fully_cleared_batch_does_not_reappear_under_a_looser_still_triggering_budget() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let mut plan = SendPlan::new();
    let first = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert_eq!(first, ids[..7].to_vec(), "先确认第一次是把可及区一次全清");
    plan.clear_tool_results(first);

    // 第一次：用量比 90%（离 85% 的触发线有 5 个点余量）；第二次：86%，只比
    // 触发线高 1 个点，明显更宽松，但依然满足「超过」。
    let second = tool_results_to_clear(&history, &plan, Some(86), Some(100), params);
    assert!(
        second.is_empty(),
        "已清的批次不该在预算变宽松之后回到清单里，实际返回 {second:?}"
    );
}

/// 只清一半，第二次重算必须**恰好**返回剩下那一半——不是「碰巧总数对得上」，
/// 逐个 id 都要对：已清的那半个不出现，没清的那半个完整出现。
#[test]
fn partially_cleared_batch_only_returns_the_remainder() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let mut plan = SendPlan::new();
    let first = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert_eq!(first, ids[..7].to_vec());

    let (already_cleared, remainder) = first.split_at(4);
    plan.clear_tool_results(already_cleared.to_vec());

    let second = tool_results_to_clear(&history, &plan, Some(86), Some(100), params);
    assert_eq!(second, remainder.to_vec());
    for id in already_cleared {
        assert!(!second.contains(id), "{id:?} 已经清过，不该再出现");
    }
}
