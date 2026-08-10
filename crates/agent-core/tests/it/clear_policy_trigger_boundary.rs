//! 102 验收：触发线边界——低于/恰好等于/刚超过触发线，以及
//! `context_window`/`last_prompt_tokens` 缺失时的行为。
//!
//! **反向锁是这个文件最要紧的一条**（`below_threshold_...`）：用量在触发线以下
//! 时，哪怕有一大堆够得着的工具结果，也必须返回空。只测「触发后清得对」的话，
//! 一个「每轮都清」的实现照样能把本文件之外的所有测试都过掉——096 决策记录
//! 把这条漏洞的代价写得很清楚：不报错、不 panic，只是每一轮都全价。
//!
//! `context_window`/`protect_recent_turns` 用 `window = 100` 而不是更大的数：
//! 分母是 100 时，百分比计算是整数精确的，不会有截断歧义混进边界断言里。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

/// 10 轮，保护最近 3 轮 → 7 轮可及。够得着的东西不少，才能让「反向锁漏了」这类
/// bug 露出来——如果可及集合本来就是空的，触发与否根本无法从结果里分辨。
const TURNS: usize = 10;
const PROTECT: usize = 3;

/// 反向锁：用量在触发线（85%）以下，哪怕有 7 轮够得着的工具结果，一次都不清。
#[test]
fn below_threshold_returns_empty_despite_reachable_results() {
    let (history, _ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(80), Some(100), params);
    assert!(
        cleared.is_empty(),
        "用量 80% < 触发线 85%，不该清任何东西，实际清了 {cleared:?}"
    );
}

/// 边界要有确定的一边：恰好等于触发线不触发（接口写的是「超过」）。
#[test]
fn exactly_at_threshold_does_not_trigger() {
    let (history, _ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(85), Some(100), params);
    assert!(
        cleared.is_empty(),
        "用量恰好等于触发线，不该触发，实际清了 {cleared:?}"
    );
}

/// 刚超过触发线：一次全清，保护区之外的 7 轮一个不剩。
#[test]
fn just_above_threshold_triggers_full_clear() {
    let (history, ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, Some(86), Some(100), params);
    let expected: Vec<_> = ids[..TURNS - PROTECT].to_vec();
    assert_eq!(cleared, expected);
}

/// `context_window: None` = 未知/不设限，一次都不触发，且不 panic。
#[test]
fn context_window_none_returns_empty_without_panic() {
    let (history, _ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    // 特意传一个大到离谱的 token 数：如果实现在算比例前没先判 `None`，这里最
    // 容易先在除法或比较上暴露问题，而不是安安静静走到「不触发」这条路。
    let cleared = tool_results_to_clear(&history, &plan, Some(u32::MAX), None, params);
    assert!(cleared.is_empty());
}

/// `last_prompt_tokens: None` = 这一轮没有观测（首轮，或这家 provider 没报），
/// 不触发。
#[test]
fn last_prompt_tokens_none_returns_empty() {
    let (history, _ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let cleared = tool_results_to_clear(&history, &plan, None, Some(100), params);
    assert!(cleared.is_empty());
}

/// 防御性场景，接口文档没有明写但「不许 `unwrap`」的精神覆盖到这里：分母是
/// `Some(0)` 时不能除零 panic。不断言具体清了什么——这个输入形状本来就没有
/// 良定义的「用量百分比」，只断言「跑得完」。
#[test]
fn context_window_zero_does_not_panic() {
    let (history, _ids) = history_with_turns(TURNS);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let _ = tool_results_to_clear(&history, &plan, Some(50), Some(0), params);
}
