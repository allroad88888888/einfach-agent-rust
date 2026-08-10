//! 102 验收：「本档只吃 `ToolResult`」——用户消息、`Thinking`、`Text` 块永远
//! 不在返回值里，且一次 `ToolUse` 没配上 `ToolResult`（还在飞/没留下结果）不该
//! 被当成「可清的一条」。
//!
//! 返回类型是 `Vec<ToolCallId>`，类型系统本身已经排除了「返回一个 Text 块」
//! 这种形状；这里要抓的是更隐蔽的问题——额外的 `Thinking`/`Text` 块会不会
//! 干扰「数轮」或「数 id」，把不该出现的 id 挤出来，或者把该出现的 id 漏掉。

use std::sync::Arc;

use agent_core::compaction::tool_results_to_clear;
use agent_core::{
    ContentBlock, DEFAULT_TRIGGER_PERCENT, Message, MessageId, Role, SendPlan, ToolCallId,
};

use crate::clear_policy_fixture::{assistant_tool_turn, clear_params, user_msg};

const PROTECT: usize = 3;

fn thinking_then_tool_message(id: u64, call_id: &str, result_text: &str) -> Message {
    let mut msg = assistant_tool_turn(id, call_id, result_text);
    msg.blocks.insert(0, ContentBlock::Thinking(Arc::from("盘算一下")));
    msg
}

fn text_only_message(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

fn tool_use_only_message(id: u64, call_id: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![ContentBlock::ToolUse {
            id: ToolCallId::new(call_id),
            name: Arc::from("srv:fs/read"),
            input: Arc::new(serde_json::json!({"path": "/tmp/a"})),
        }],
    }
}

/// 5 轮，保护最近 3 轮 → 轮 0、1 可及。轮 0 里混了 `Thinking` 块和一条紧随其后
/// 的纯 `Text` 助手消息（同一轮内，没有新 `User` 消息，不该被数成新的一轮）。
#[test]
fn thinking_and_text_blocks_do_not_produce_or_hide_ids() {
    let mut history = imbl::Vector::new();
    let mut next_id = 1u64;

    // 轮 0：Thinking + ToolUse + ToolResult，外加一条尾随的纯文本总结消息。
    history.push_back(user_msg(next_id, "轮 0"));
    next_id += 1;
    history.push_back(thinking_then_tool_message(next_id, "call_0", "result_0"));
    next_id += 1;
    history.push_back(text_only_message(next_id, "以上是我做的事"));
    next_id += 1;

    // 轮 1：普通一对。
    history.push_back(user_msg(next_id, "轮 1"));
    next_id += 1;
    history.push_back(assistant_tool_turn(next_id, "call_1", "result_1"));
    next_id += 1;

    // 轮 2、3、4：普通一对，落在保护区内，内容无所谓。
    for i in 2..5 {
        history.push_back(user_msg(next_id, &format!("轮 {i}")));
        next_id += 1;
        let call_id = format!("call_{i}");
        history.push_back(assistant_tool_turn(next_id, &call_id, &format!("result_{i}")));
        next_id += 1;
    }

    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);
    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);

    assert_eq!(
        cleared,
        vec![
            ToolCallId::new("call_0"),
            ToolCallId::new("call_1"),
        ]
    );
}

/// 6 轮，保护最近 3 轮 → 轮 0、1、2 可及。轮 1 只有 `ToolUse`、没有配上
/// `ToolResult`（模拟还在飞/没留下结果的调用）：它落在可及区，但没有结果可清，
/// 不该出现在清单里——顺序也验证了跳过它之后轮 2 紧接轮 0，不留洞。
#[test]
fn tool_use_without_matching_tool_result_is_never_selected() {
    let mut history = imbl::Vector::new();
    let mut next_id = 1u64;

    history.push_back(user_msg(next_id, "轮 0"));
    next_id += 1;
    history.push_back(assistant_tool_turn(next_id, "call_0", "result_0"));
    next_id += 1;

    history.push_back(user_msg(next_id, "轮 1"));
    next_id += 1;
    history.push_back(tool_use_only_message(next_id, "call_1"));
    next_id += 1;

    history.push_back(user_msg(next_id, "轮 2"));
    next_id += 1;
    history.push_back(assistant_tool_turn(next_id, "call_2", "result_2"));
    next_id += 1;

    for i in 3..6 {
        history.push_back(user_msg(next_id, &format!("轮 {i}")));
        next_id += 1;
        let call_id = format!("call_{i}");
        history.push_back(assistant_tool_turn(next_id, &call_id, &format!("result_{i}")));
        next_id += 1;
    }

    let plan = SendPlan::new();
    let params = clear_params(DEFAULT_TRIGGER_PERCENT, PROTECT);
    let cleared = tool_results_to_clear(&history, &plan, Some(90), Some(100), params);

    assert_eq!(
        cleared,
        vec![
            ToolCallId::new("call_0"),
            ToolCallId::new("call_2"),
        ]
    );
}
