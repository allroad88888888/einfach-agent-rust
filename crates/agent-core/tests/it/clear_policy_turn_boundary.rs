//! 102 验收：「轮」的边界——**这条边界最容易写错**（issue 原话）。
//!
//! 定义（102 §「轮」怎么数）：一条 `Role::User` 消息开启一轮；「最近 N 轮」=
//! 从倒数第 N 条 `User` 消息（含）到历史末尾；`User` 消息不足 N 条时**整个历史
//! 都在保护区，返回空**。
//!
//! 每个用例都把触发条件钉死在「明显超过触发线」（90% > 85%），这样任何非空/
//! 空的结果差异都只能归因于轮边界的计算，不会跟触发线搅在一起。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

const PROTECT: usize = 3;

fn triggering_cleared(n_turns: usize) -> Vec<agent_core::ToolCallId> {
    let (history, _ids) = history_with_turns(n_turns);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);
    tool_results_to_clear(&history, &plan, Some(90), Some(100), params)
}

/// 空历史：连一条 `User` 消息都没有，不该 panic，也没有东西可清。
#[test]
fn zero_turns_returns_empty() {
    assert!(triggering_cleared(0).is_empty());
}

/// `User` 消息数（2）**不足** N（3 轮的保护区）：整个历史都在保护区，返回空
/// ——即便用量早就超过触发线。
#[test]
fn fewer_than_protect_turns_protects_entire_history() {
    assert!(triggering_cleared(2).is_empty());
}

/// `User` 消息数**恰好等于** N：倒数第 N 条 `User` 消息就是第 1 条，保护区的
/// 起点落在历史开头——「不足」和「恰好」两种原因不同，结果都得是空，这条专门
/// 卡住「>= N 就以为有东西可清」的错法。
#[test]
fn exactly_protect_turns_still_protects_entire_history() {
    assert!(triggering_cleared(3).is_empty());
}

/// N + 1 轮：只有最老的第 0 轮落在保护区之外，其余 3 轮（最近 N 轮）原封不动。
#[test]
fn one_turn_more_than_protect_exposes_only_the_oldest_turn() {
    let (history, ids) = history_with_turns(4);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert_eq!(cleared, vec![ids[0].clone()]);
}

/// N + 2 轮：最老的两轮落在保护区之外，验证边界不是「N+1 时对、再往后就漂」的
/// 一次性巧合。
#[test]
fn two_turns_more_than_protect_exposes_two_oldest_turns() {
    let (history, ids) = history_with_turns(5);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert_eq!(cleared, vec![ids[0].clone(), ids[1].clone()]);
}
