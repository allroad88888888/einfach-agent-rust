//! 102 验收：把 `tool_results_to_clear` 的输出写进 `SendPlan` 再投影
//! （099 的 `project`），`ToolUse` 与 `ToolResult` 的 id 集合必须恒等。
//!
//! 099 的做法（换占位而不是删块）已经保证配对不破，这里不是重新验那件事，
//! 是**回归锁**——防止有人在 102 这一侧改了「选哪些 id 去清」的逻辑，
//! 顺手选出了一个只有 `ToolUse` 没有 `ToolResult`（或反过来）的 id，
//! 结果 099 那层的不变量被 102 的输入破坏。

use std::collections::BTreeSet;

use agent_core::compaction::tool_results_to_clear;
use agent_core::value::send_plan::project;
use agent_core::{CLEARED_TOOL_RESULT, ContentBlock, DEFAULT_TRIGGER_PERCENT, SendPlan};

use crate::clear_policy_fixture::{clear_params, history_with_turns};

#[test]
fn projected_tool_use_and_tool_result_ids_stay_paired_after_clear_policy_selection() {
    let (history, ids) = history_with_turns(10);
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, 3);

    let selected = tool_results_to_clear(&history, &SendPlan::new(), Some(90), Some(100), params);
    assert_eq!(selected, ids[..7].to_vec(), "先确认这次选中了非空的一批");

    let mut plan = SendPlan::new();
    plan.clear_tool_results(selected.clone());

    let projected = project(&history, &plan, None);

    let mut tool_use_ids = BTreeSet::new();
    let mut tool_result_ids = BTreeSet::new();
    for msg in &projected {
        for block in &msg.blocks {
            match block {
                ContentBlock::ToolUse { id, .. } => {
                    tool_use_ids.insert(id.clone());
                }
                ContentBlock::ToolResult { id, .. } => {
                    tool_result_ids.insert(id.clone());
                }
                _ => {}
            }
        }
    }

    assert_eq!(
        tool_use_ids, tool_result_ids,
        "投影之后 ToolUse 与 ToolResult 的 id 集合必须恒等，不能出现落单的一半"
    );
    assert_eq!(tool_use_ids.len(), 10, "10 轮各一次调用，块的总数不该变");

    // 顺带确认清除确实换成了占位，没被清的仍是原文——这样上面的 id 集合相等
    // 不是靠「反正块都还在」蒙混过去的。
    let selected_set: BTreeSet<_> = selected.iter().cloned().collect();
    for msg in &projected {
        for block in &msg.blocks {
            if let ContentBlock::ToolResult { id, content, .. } = block {
                if selected_set.contains(id) {
                    assert_eq!(content.as_ref(), CLEARED_TOOL_RESULT);
                } else {
                    assert_ne!(content.as_ref(), CLEARED_TOOL_RESULT);
                }
            }
        }
    }
}
