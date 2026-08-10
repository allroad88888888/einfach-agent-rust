//! 102 验收：纯函数（红线 1）——同一份入参算 1000 次，输出逐项相同，顺序也
//! 相同。这条抓的是「读了点不该读的东西」（时钟、哈希迭代顺序之类），单次调用
//! 看不出来，重复一千次跑到的才有区分力。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

#[test]
fn same_input_yields_byte_identical_output_across_a_thousand_calls() {
    let (history, ids) = history_with_turns(12);
    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);

    let first = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
    assert!(!first.is_empty(), "先确认这份入参真的会触发，测试才有意义");
    assert_eq!(first, ids[..9].to_vec());

    for i in 0..1000 {
        let again = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);
        assert_eq!(again, first, "第 {i} 次调用的结果跟第一次不一致");
    }
}
