//! 099 验收：投影纯函数 `project` 在**边界与摘要**这两档（第 3、4 档）上的
//! 行为，以及「投影后空消息整条不出现」这条通用规则。清工具结果那一档
//! （第 2 档）见 `send_plan_project_clearing.rs`。

use std::sync::Arc;

use imbl::Vector;

use agent_core::ids::{MessageId, SummaryId};
use agent_core::value::message::{ContentBlock, Message, Role};
use agent_core::value::send_plan::{SendPlan, project};

fn build_history(messages: Vec<Message>) -> Vector<Message> {
    let mut v: Vector<Message> = Vector::new();
    for m in messages {
        v.push_back(m);
    }
    v
}

fn user_text(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::User,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

fn assistant_text(id: u64, text: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![ContentBlock::Text(Arc::from(text))],
    }
}

/// 投影后所有块都没了的消息整条不出现——空消息发出去是 400。
#[test]
fn project_drops_messages_whose_blocks_are_entirely_empty() {
    let history = build_history(vec![
        user_text(1, "hi"),
        Message {
            id: MessageId(2),
            role: Role::Assistant,
            blocks: vec![],
        },
        assistant_text(3, "done"),
    ]);

    let plan = SendPlan::new();
    let projected = project(&history, &plan, None);

    assert_eq!(projected.len(), 2, "空消息要整条被丢弃");
    assert!(projected.iter().all(|m| !m.blocks.is_empty()));
    assert_eq!(projected[0].id, MessageId(1));
    assert_eq!(projected[1].id, MessageId(3));
}

/// `plan.summary()` 是 `Some` 但 `summary_text` 传 `None`：边界不生效，宁可
/// 多发完整历史，也不可发一段引用不到正文的空洞。
#[test]
fn project_boundary_not_applied_when_summary_text_missing() {
    let history = build_history(vec![
        user_text(1, "old message"),
        user_text(2, "another old message"),
        user_text(3, "recent message"),
    ]);

    let mut plan = SendPlan::new();
    plan.advance_boundary(2, Some(SummaryId::new("sum_1")))
        .unwrap();

    let projected = project(&history, &plan, None);
    let expected: Vec<Message> = history.iter().cloned().collect();
    assert_eq!(projected, expected);
}

/// 第 4 档「清窗口」：边界前进但没有摘要（`summary` 是 `None`）——边界照样
/// 生效，边界之前的消息不出现，不需要摘要来解锁这一档。
#[test]
fn project_boundary_excludes_earlier_messages_without_a_summary() {
    let history = build_history(vec![
        user_text(1, "old 1"),
        user_text(2, "old 2"),
        user_text(3, "recent 1"),
        user_text(4, "recent 2"),
    ]);

    let mut plan = SendPlan::new();
    plan.advance_boundary(2, None).unwrap();

    let projected = project(&history, &plan, None);
    let expected: Vec<Message> = history.iter().skip(2).cloned().collect();
    assert_eq!(projected, expected);
}

/// 摘要引用非空、摘要正文也给到了：摘要作为一条消息出现在最前面，边界之前
/// 的原始消息不再原样出现。
#[test]
fn project_prepends_summary_message_when_present() {
    let history = build_history(vec![
        user_text(1, "old 1"),
        user_text(2, "old 2"),
        user_text(3, "recent"),
    ]);

    let mut plan = SendPlan::new();
    plan.advance_boundary(2, Some(SummaryId::new("sum_1")))
        .unwrap();

    let summary_text: Arc<str> = Arc::from("摘要：讨论了 old 1 和 old 2。");
    let projected = project(&history, &plan, Some(&summary_text));

    assert_eq!(projected.len(), 2, "摘要消息 + 边界之后剩的一条");
    let first_is_summary = projected[0]
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text(t) if t.as_ref() == summary_text.as_ref()));
    assert!(first_is_summary, "摘要要作为最前面的一条消息出现");
    assert_eq!(
        projected[1].id,
        MessageId(3),
        "边界之前的消息不再原样出现，只剩边界之后的部分跟在摘要后面"
    );
}

/// 反过来的边界情况：`summary_text` 给了，但 `plan.summary()` 是 `None`
/// （比如只推进过边界、没绑摘要引用）——不该凭空插入一条摘要消息。
#[test]
fn project_ignores_summary_text_when_plan_has_no_summary_reference() {
    let history = build_history(vec![user_text(1, "a"), user_text(2, "b")]);

    let plan = SendPlan::new();
    let phantom_summary: Arc<str> = Arc::from("不该出现的摘要");

    let projected = project(&history, &plan, Some(&phantom_summary));
    let expected: Vec<Message> = history.iter().cloned().collect();
    assert_eq!(
        projected, expected,
        "plan 没有摘要引用时，summary_text 不该被凭空插入一条消息"
    );
}
