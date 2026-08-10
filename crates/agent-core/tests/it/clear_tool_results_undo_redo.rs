//! 101 验收核心四条，同一个 fixture 一次走完：
//!
//! - 清 50 条工具结果之后，投影（099 的纯函数，充当「下一轮 encode 会看到什么」
//!   的免 IO 代理）里这 50 条正文变成 [`CLEARED_TOOL_RESULT`]
//! - 这次清除落地的那条 entry，`prev` 序列化后 < 1 KB
//! - `undo_step` 一次，50 条正文原样回来
//! - `redo_step` 一次，50 条重新消失
//! - 全程 `messages_of` 的条数不变——清除只改 `SendPlan`，不碰完整记录
//!
//! 不读 `clear_tool_results` 的实现体，只喂公开签名（101「定死的接口」），
//! 用 099/100 已经钉死的 `project` 当验证探针。

use std::collections::BTreeMap;

use agent_core::value::send_plan::project;
use agent_core::{AgentId, AtomKey, CLEARED_TOOL_RESULT, ContentBlock, Message, Slot, ToolCallId, UndoReport};

use crate::clear_tool_results_fixture::session_with_n_tool_calls;

const N: usize = 50;

/// 投影结果里每个 `ToolCallId` 对应的 `ToolResult.content`（fixture 保证一个
/// call_id 只出现一次)。
fn tool_result_contents(messages: &[Message]) -> BTreeMap<ToolCallId, std::sync::Arc<str>> {
    messages
        .iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { id, content, .. } => Some((id.clone(), content.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn clear_fifty_then_undo_then_redo_round_trips_through_projection() {
    let (mut session, ids) = session_with_n_tool_calls(N);
    assert_eq!(ids.len(), N);
    let root = AgentId::root();

    let messages_before_clear = session.messages_of(&root).len();

    // 清除之前：投影就是完整历史，50 条正文原样在。
    let history = session.messages_of(&root);
    let before = tool_result_contents(&project(&history, &session.send_plan_of(&root), None));
    for (i, call_id) in ids.iter().enumerate() {
        assert_eq!(
            before.get(call_id).map(|c| c.as_ref()),
            Some(format!("result_{i}").as_str()),
            "清除之前第 {i} 条工具结果该是原文"
        );
    }

    let outcome = session.clear_tool_results(&root, ids.iter().cloned());
    assert_eq!(
        outcome.newly_cleared, ids,
        "一次性清 50 个，全部该记进 newly_cleared，且保持传入顺序"
    );
    assert!(outcome.already_cleared.is_empty());
    assert!(outcome.unknown.is_empty());

    // ---- 验收：该 entry 的 prev 序列化后 < 1 KB ----
    let entry = session.last_entry().expect("清除应该留下一条 entry");
    let send_plan_changes: Vec<_> = entry
        .changes
        .iter()
        .filter(|c| matches!(&c.key, AtomKey::Agent(a, Slot::SendPlan) if *a == root))
        .collect();
    assert_eq!(
        send_plan_changes.len(),
        1,
        "清除只该产出一条 SendPlan 的变更——内容一个字节不删，改的只是发不发"
    );
    let change = send_plan_changes[0];
    let prev_bytes = serde_json::to_vec(&change.prev).expect("AgentValue 全员可序列化（红线 3）");
    assert!(
        prev_bytes.len() < 1024,
        "prev 序列化后应该 < 1KB，实际 {} 字节",
        prev_bytes.len()
    );

    // ---- 清除之后：投影里 50 条正文全变成占位，ToolUse 侧不受影响 ----
    let history_after_clear = session.messages_of(&root);
    let after_clear =
        tool_result_contents(&project(&history_after_clear, &session.send_plan_of(&root), None));
    for call_id in &ids {
        assert_eq!(
            after_clear.get(call_id).map(|c| c.as_ref()),
            Some(CLEARED_TOOL_RESULT),
            "清除之后 {call_id:?} 的正文该变成占位文本"
        );
    }

    // ---- 完整记录一条没少：消息条数全程不变 ----
    assert_eq!(
        session.messages_of(&root).len(),
        messages_before_clear,
        "清除只改 SendPlan，不该动 Messages 槽位本身的条数"
    );

    // ---- undo 一次：50 条正文原样回来 ----
    let undo_report = session.undo_step();
    assert!(
        matches!(undo_report, UndoReport::Applied { entries: 1, .. }),
        "{undo_report:?}"
    );
    assert!(
        session.send_plan_of(&root).is_pristine(),
        "撤销清除之后该退回没压缩过的样子"
    );

    let history_after_undo = session.messages_of(&root);
    let after_undo =
        tool_result_contents(&project(&history_after_undo, &session.send_plan_of(&root), None));
    for (i, call_id) in ids.iter().enumerate() {
        assert_eq!(
            after_undo.get(call_id).map(|c| c.as_ref()),
            Some(format!("result_{i}").as_str()),
            "undo 一次之后第 {i} 条正文该原样回来：{call_id:?}"
        );
    }
    assert_eq!(
        session.messages_of(&root).len(),
        messages_before_clear,
        "undo 不该改消息条数"
    );

    // ---- redo 一次：50 条重新消失 ----
    let redo_report = session.redo_step();
    assert!(
        matches!(redo_report, UndoReport::Applied { entries: 1, .. }),
        "{redo_report:?}"
    );
    let history_after_redo = session.messages_of(&root);
    let after_redo =
        tool_result_contents(&project(&history_after_redo, &session.send_plan_of(&root), None));
    for call_id in &ids {
        assert_eq!(
            after_redo.get(call_id).map(|c| c.as_ref()),
            Some(CLEARED_TOOL_RESULT),
            "redo 一次之后 {call_id:?} 该重新变成占位文本"
        );
    }
    assert_eq!(
        session.messages_of(&root).len(),
        messages_before_clear,
        "redo 也不该改消息条数"
    );
}
