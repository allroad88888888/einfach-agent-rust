//! 096/102 联合验收：造一个稳定增长的会话跑 30 轮，第 2 档触发次数 **≤ 2**；
//! 每次触发后，保护区之外的工具结果一个不剩、保护区之内的一个没动。
//!
//! 用量模型：每轮新增一次工具调用，未清的工具结果每条贡献固定 token 数——
//! 触发后一次全清会让用量应声跌落，下一次触发要等新工具结果重新堆到线上，
//! 于是 30 轮里只触发两次，不是每轮都改。这条区别于 `clear_policy_trigger_
//! boundary.rs` 的静态边界测试：这里是一段会话的**时间线**，专门盯着「先清
//! 一次之后还会不会不该触发的时候又触发」这类只在多轮里才冒头的问题。

use std::collections::BTreeSet;

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan, ToolCallId};

use crate::clear_policy_fixture::{clear_params, push_turn};

const ROUNDS: usize = 30;
const PROTECT: usize = 3;
/// `window = 100`：百分比就是 token 数本身，没有整数截断歧义（跟
/// `clear_policy_trigger_boundary.rs` 同一个理由）。
const WINDOW: u32 = 100;
/// 每条**未清**工具结果贡献的 token 数——一清就归零，模拟真实的「换成占位」。
const TOKENS_PER_UNCLEARED: u32 = 6;

#[test]
fn thirty_round_growing_session_triggers_at_most_twice() {
    let mut history = imbl::Vector::new();
    let mut next_id = 1u64;
    let mut plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);

    let mut ids: Vec<ToolCallId> = Vec::with_capacity(ROUNDS);
    let mut trigger_count = 0usize;

    for round in 0..ROUNDS {
        let call_id = format!("call_{round}");
        ids.push(push_turn(&mut history, &mut next_id, &call_id));
        let total_turns = ids.len();

        let uncleared = total_turns - plan.cleared().len();
        let last_prompt_tokens = TOKENS_PER_UNCLEARED * uncleared as u32;

        let cleared = tool_results_to_clear(
            &history,
            &plan,
            Some(last_prompt_tokens),
            Some(WINDOW),
            params,
        );

        if cleared.is_empty() {
            continue;
        }
        trigger_count += 1;

        let boundary = total_turns - PROTECT;
        let reachable: BTreeSet<_> = ids[..boundary].iter().cloned().collect();
        let protected: BTreeSet<_> = ids[boundary..].iter().cloned().collect();

        plan.clear_tool_results(cleared);
        let cleared_now: BTreeSet<_> = plan.cleared().iter().cloned().collect();

        assert_eq!(
            cleared_now, reachable,
            "第 {trigger_count} 次触发（第 {round} 轮）之后，保护区之外的工具结果该一个不剩"
        );
        assert!(
            cleared_now.is_disjoint(&protected),
            "第 {trigger_count} 次触发（第 {round} 轮）之后，最近 {PROTECT} 轮的工具结果该一个没动"
        );
    }

    assert!(
        (1..=2).contains(&trigger_count),
        "30 轮里第 2 档该触发 1~2 次（≤2 是验收线），实际触发了 {trigger_count} 次"
    );
    // 用量模型是精确设计的（首次在第 15 轮过线，二次在第 27 轮），这里把它钉死
    // 成一条更强的断言：既不是「从没触发」也不是「每轮都触发」。
    assert_eq!(trigger_count, 2);
}
