//! 102 验收：已清列表按**在历史中出现的先后**排列（最老在前），**不是** id
//! 字典序——红线 11 管的是「进 prompt 的字节序确定」，不是「哪种排序都行只要
//! 确定」。故意把最老的一轮起一个字典序最大的 id，最新可及的一轮起一个字典序
//! 最小的 id：两种排序会给出相反的结果，混进去的「顺手 sort 一下」当场露馅。

use agent_core::compaction::tool_results_to_clear;
use agent_core::{DEFAULT_TRIGGER_PERCENT, SendPlan, ToolCallId};

use crate::clear_policy_fixture::{clear_params, push_turn};

const PROTECT: usize = 3;

#[test]
fn output_order_is_chronological_not_lexical() {
    let mut history = imbl::Vector::new();
    let mut next_id = 1u64;

    // 可及区（3 轮，历史顺序 zzz → mmm → aaa）：字典序恰好是历史顺序的倒转。
    push_turn(&mut history, &mut next_id, "zzz");
    push_turn(&mut history, &mut next_id, "mmm");
    push_turn(&mut history, &mut next_id, "aaa");
    // 保护区（3 轮）：内容无所谓，只要存在、把前三轮推出保护区。
    push_turn(&mut history, &mut next_id, "p0");
    push_turn(&mut history, &mut next_id, "p1");
    push_turn(&mut history, &mut next_id, "p2");

    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);
    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);

    let chronological = vec![
        ToolCallId::new("zzz"),
        ToolCallId::new("mmm"),
        ToolCallId::new("aaa"),
    ];
    let lexical = {
        let mut v = chronological.clone();
        v.sort();
        v
    };

    assert_ne!(
        chronological, lexical,
        "这组 id 必须让两种排序给出不同结果，测试才分得开谁对谁错"
    );
    assert_eq!(
        cleared, chronological,
        "顺序必须是历史出现顺序（最老在前），不是 id 字典序"
    );
}
