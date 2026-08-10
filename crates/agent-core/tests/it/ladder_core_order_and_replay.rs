//! 108 验收：`compaction::next_action` 的判定顺序 + 重放确定性。
//!
//! 纯函数层面直接测，不经 `run_turn`——`next_action` 的入参跟 102 的
//! `tool_results_to_clear` 同一套（`history` / `plan` / `last_prompt_tokens` /
//! `context_window` / `ClearParams`），复用 `clear_policy_fixture` 的
//! `history_with_turns(10)` 夹具：10 轮工具调用，`protect_recent_turns=3` 时
//! 保护区起点是**第 7 轮的位置 = 14**（`ids[..7]` 落在保护区外），这个数字已经
//! 被 `clear_policy_project_integration.rs` 独立核实过（`selected == ids[..7]`），
//! 这里直接借用不重新推导。
//!
//! 判定顺序（108「定死的接口」）：
//! 1. 压力没超 `trigger_percent` → `Nothing`
//! 2. 超了，`tool_results_to_clear` 非空 → `ClearToolResults`（第 2 档优先，永远）
//! 3. 超了，`tool_results_to_clear` 空 → `Summarize { upto: 保护区起点 }`；
//!    保护区起点为 0（没东西可摘）时 → `Nothing`

use agent_core::compaction::{LadderAction, next_action, tool_results_to_clear};
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

/// 10 轮夹具在 `protect_recent_turns=3` 下的保护区起点（消息下标）——
/// 第 7 轮（0-indexed）的位置 = 7 * 2。
const UPTO_AFTER_SEVEN_TURNS: usize = 14;

/// 判定顺序第 2 步：超了且 `tool_results_to_clear` 非空 → 第 2 档，**永远优先**，
/// 即使这一轮同时满足「保护区起点非零」（第 3 档的前提也成立）。
#[test]
fn tier2_is_preferred_over_tier3_whenever_it_has_something_to_clear() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();

    let expected = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert_eq!(expected, ids[..7].to_vec(), "先确认这份夹具的第 2 档产出非空");

    let action = next_action(&history, &plan, Some(90), Some(100), params);
    assert_eq!(
        action,
        LadderAction::ClearToolResults(expected),
        "第 2 档有东西可清时必须选它，不能因为保护区起点非零就跳去摘要"
    );
}

/// 判定顺序第 3 步：第 2 档已经清空（都在 `plan.cleared()` 里了）才轮到第 3 档，
/// `upto` 等于保护区起点。
#[test]
fn tier3_fires_only_after_tier2_is_exhausted() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let mut plan = SendPlan::new();
    plan.clear_tool_results(ids[..7].to_vec());

    let still_clearable = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert!(still_clearable.is_empty(), "先确认第 2 档这次真的清空了");

    let action = next_action(&history, &plan, Some(90), Some(100), params);
    assert_eq!(
        action,
        LadderAction::Summarize {
            upto: UPTO_AFTER_SEVEN_TURNS
        },
        "第 2 档清空之后该轮到第 3 档，upto 落在保护区起点"
    );
}

/// 第 3 步的例外：第 2 档清空了，但保护区起点本身是 0（历史太短，没东西可摘）
/// → `Nothing`，不是 `Summarize { upto: 0 }`。
#[test]
fn nothing_when_tier2_is_exhausted_but_protected_region_start_is_zero() {
    let (history, _ids) = history_with_turns(2); // 少于 protect_recent_turns=3
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();

    let clearable = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert!(clearable.is_empty(), "历史太短，整个都在保护区内，天然清不动");

    let action = next_action(&history, &plan, Some(90), Some(100), params);
    assert_eq!(
        action,
        LadderAction::Nothing,
        "没东西可摘时不该产出 upto=0 的 Summarize，该是 Nothing"
    );
}

/// 判定顺序第 1 步的反向锁：压力没超过触发线，即使有大量够得着的工具结果，
/// 一档都不该开火——同 102 的反向锁，漏了这条会变成每轮改中段、每轮全价。
#[test]
fn nothing_when_below_the_trigger_line_even_with_reachable_results() {
    let (history, _ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();

    let action = next_action(&history, &plan, Some(50), Some(100), params);
    assert_eq!(
        action,
        LadderAction::Nothing,
        "50% 没过 85% 触发线，不该有任何一档开火"
    );
}

/// `context_window: None` → 两档都不触发（096 第一问：不许 `unwrap`，也不许瞎猜）。
#[test]
fn nothing_when_context_window_is_none() {
    let (history, _ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();

    let action = next_action(&history, &plan, Some(999_999), None, params);
    assert_eq!(action, LadderAction::Nothing);
}

/// `last_prompt_tokens: None`（首轮，还没有实测值）→ 不触发。
#[test]
fn nothing_when_last_prompt_tokens_is_none() {
    let (history, _ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();

    let action = next_action(&history, &plan, None, Some(100), params);
    assert_eq!(action, LadderAction::Nothing);
}

/// 「同一份历史重放两次，压缩决定逐字节相同」——第 2 档场景，跑 1000 次。
#[test]
fn replaying_the_tier2_decision_a_thousand_times_is_byte_identical() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let plan = SendPlan::new();
    let expected = LadderAction::ClearToolResults(ids[..7].to_vec());

    for _ in 0..1000 {
        let action = next_action(&history, &plan, Some(90), Some(100), params);
        assert_eq!(action, expected, "第 2 档场景的决定必须逐次相同，顺序也不能变");
    }
}

/// 同上，第 3 档场景。
#[test]
fn replaying_the_tier3_decision_a_thousand_times_is_byte_identical() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);
    let mut plan = SendPlan::new();
    plan.clear_tool_results(ids[..7].to_vec());
    let expected = LadderAction::Summarize {
        upto: UPTO_AFTER_SEVEN_TURNS,
    };

    for _ in 0..1000 {
        let action = next_action(&history, &plan, Some(90), Some(100), params);
        assert_eq!(action, expected, "第 3 档场景的决定必须逐次相同");
    }
}
