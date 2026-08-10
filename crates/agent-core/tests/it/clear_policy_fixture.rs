//! `clear_policy_*` 系列共用的历史构造器：直接拼 `Vector<Message>`，不经过
//! `Session` 状态机——102 的 `tool_results_to_clear` 是纯函数，吃的就是这个
//! 类型，直接构造能精确控制 token 数、id 命名、块类型这些边界条件，比驱动
//! 一遍完整会话状态机更直接。只负责「造一份确定的输入」，不含断言——跟
//! `support/mod.rs` 顶部、`clear_tool_results_fixture.rs` 同一条纪律。
//!
//! 独立于 `clear_tool_results_fixture.rs`（101 的脚手架）：那份构造的是
//! `Session`，这份构造的是裸 `Vector<Message>`，两者输入形状不同，硬凑一个
//! 共用没有意义。

#![allow(dead_code)]

use std::sync::Arc;

use imbl::Vector;
use serde_json::json;

use agent_core::{ClearParams, ContentBlock, Message, MessageId, Role, ToolCallId};

/// 一条 `Role::User` 消息——开启一轮。
pub fn user_msg(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

/// 一条 `Role::Assistant` 消息，携带恰好一对 `ToolUse`/`ToolResult`。
pub fn assistant_tool_turn(id: u64, call_id: &str, result_text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![
            ContentBlock::ToolUse {
                id: ToolCallId::new(call_id),
                name: Arc::from("srv:fs/read"),
                input: Arc::new(json!({"path": "/tmp/a"})),
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new(call_id),
                content: Arc::from(result_text),
                is_error: false,
            },
        ],
    }
}

/// 往 `history` 追加一整轮：一条 user 消息 + 一次工具调用。`call_id` 由调用方
/// 指定——排序测试要故意造出跟历史顺序不一致的字典序，不能让这里替它铸造。
pub fn push_turn(history: &mut Vector<Message>, next_id: &mut u64, call_id: &str) -> ToolCallId {
    let uid = *next_id;
    *next_id += 1;
    history.push_back(user_msg(uid, &format!("请调用 {call_id}")));

    let aid = *next_id;
    *next_id += 1;
    history.push_back(assistant_tool_turn(aid, call_id, &format!("result_{call_id}")));

    ToolCallId::new(call_id)
}

/// `n` 轮，`call_id` 依次是 `call_0..call_{n-1}`（铸造顺序 = 历史顺序 = 调用
/// 顺序），互不相同，方便测试拿 id 反查「这一条」在不在结果里。
pub fn history_with_turns(n: usize) -> (Vector<Message>, Vec<ToolCallId>) {
    let mut history = Vector::new();
    let mut next_id = 1u64;
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let call_id = format!("call_{i}");
        ids.push(push_turn(&mut history, &mut next_id, &call_id));
    }
    (history, ids)
}

/// 造一份 `ClearParams`，两个字段都显式传，避免测试里散落魔法数字的来源不明。
pub fn clear_params(trigger_percent: u32, protect_recent_turns: usize) -> ClearParams {
    ClearParams {
        trigger_percent,
        protect_recent_turns,
    }
}
