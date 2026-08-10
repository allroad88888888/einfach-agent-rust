//! 099 验收：投影纯函数 `project` 在**清工具结果**这一档（第 2 档）上的行为——
//! 确定性、恒等元、`ToolResult` 内容替换、`ToolUse`/`ToolResult` 的 id 集合
//! 恒等。边界/摘要那一档（第 3、4 档）见 `send_plan_project_boundary.rs`。

use std::collections::BTreeSet;
use std::sync::Arc;

use imbl::Vector;

use agent_core::ids::{MessageId, ToolCallId};
use agent_core::value::message::{ContentBlock, Message, Role};
use agent_core::value::send_plan::{CLEARED_TOOL_RESULT, SendPlan, project};

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

/// 一条消息里一对 `ToolUse`/`ToolResult`——issue 099 定死的形状：两者是
/// `blocks` 里的块，不是独立消息。
fn assistant_tool_pair(id: u64, call_id: &str, tool_name: &str, result: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![
            ContentBlock::ToolUse {
                id: ToolCallId::new(call_id),
                name: Arc::from(tool_name),
                input: Arc::new(serde_json::json!({})),
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new(call_id),
                content: Arc::from(result),
                is_error: false,
            },
        ],
    }
}

fn tool_use_ids(msgs: &[Message]) -> BTreeSet<ToolCallId> {
    msgs.iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_ids(msgs: &[Message]) -> BTreeSet<ToolCallId> {
    msgs.iter()
        .flat_map(|m| &m.blocks)
        .filter_map(|b| match b {
            ContentBlock::ToolResult { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

/// 同一份 `(历史, SendPlan)` 投影 1000 次，逐字节相同——不是比较两次，是真的
/// 循环 1000 次都跟第一次的序列化结果比对。纯函数没有任何借口在第 501 次
/// 漂移。
#[test]
fn project_1000_times_is_byte_identical() {
    let history = build_history(vec![
        user_text(1, "请读一下 a.txt"),
        assistant_tool_pair(2, "call_1", "srv:fs/read", "file contents"),
        user_text(3, "再读一下 b.txt"),
        assistant_tool_pair(4, "call_2", "srv:fs/read", "more contents"),
        assistant_text(5, "都读完了"),
    ]);

    let mut plan = SendPlan::new();
    plan.clear_tool_results([ToolCallId::new("call_1")]);
    plan.advance_boundary(1, Some(agent_core::ids::SummaryId::new("sum_1")))
        .unwrap();

    let summary_text: Arc<str> = Arc::from("前情提要：已经读过 a.txt 和 b.txt。");

    let first = project(&history, &plan, Some(&summary_text));
    let first_json = serde_json::to_string(&first).unwrap();
    assert!(!first_json.is_empty());

    for _ in 0..1000 {
        let projected = project(&history, &plan, Some(&summary_text));
        assert_eq!(serde_json::to_string(&projected).unwrap(), first_json);
    }
}

/// 恒等元：空 `SendPlan` 投影出的历史等于完整历史——这条保证「不压缩」不是
/// 一条特殊路径，而是投影函数在零输入下的自然结果。
#[test]
fn project_identity_element_equals_full_history() {
    let history = build_history(vec![
        user_text(1, "hi"),
        assistant_tool_pair(2, "call_1", "srv:fs/read", "contents"),
        assistant_text(3, "done"),
    ]);

    let plan = SendPlan::new();
    let projected = project(&history, &plan, None);

    let expected: Vec<Message> = history.iter().cloned().collect();
    assert_eq!(projected, expected);
}

/// 已清的 `ToolResult.content` 变成 `CLEARED_TOOL_RESULT`，`ToolUse` 块原样
/// 保留（配对天然不破——落单的 `ToolUse` 会被有的 provider 判成 400）。
#[test]
fn project_clears_tool_result_content_but_keeps_tool_use() {
    let history = build_history(vec![assistant_tool_pair(
        1,
        "call_1",
        "srv:fs/read",
        "secret contents",
    )]);

    let mut plan = SendPlan::new();
    plan.clear_tool_results([ToolCallId::new("call_1")]);

    let projected = project(&history, &plan, None);
    assert_eq!(projected.len(), 1);
    let blocks = &projected[0].blocks;
    assert_eq!(blocks.len(), 2, "ToolUse 和 ToolResult 两个块都还在");

    let tool_use_kept = blocks.iter().any(|b| {
        matches!(b, ContentBlock::ToolUse { id, .. } if *id == ToolCallId::new("call_1"))
    });
    assert!(tool_use_kept, "ToolUse 块要原样保留");

    let cleared = blocks.iter().find_map(|b| match b {
        ContentBlock::ToolResult { id, content, is_error }
            if *id == ToolCallId::new("call_1") =>
        {
            Some((content.clone(), *is_error))
        }
        _ => None,
    });
    let (content, is_error) = cleared.expect("ToolResult 块要原样保留（只换内容）");
    assert_eq!(content.as_ref(), CLEARED_TOOL_RESULT);
    assert!(!is_error, "清除只换 content，不改 is_error 本身的取值");
}

/// property 式检查：不管已清列表长什么样（清一个、清全部、清一个根本不存在
/// 的 id、同一条消息里塞了多个 `ToolUse`/`ToolResult`），投影结果里两种块的
/// id 集合必须恒等——任何输入下都不出现落单的一半。
#[test]
fn project_tool_use_and_tool_result_ids_never_go_orphan() {
    let two_pairs_separate_messages = build_history(vec![
        assistant_tool_pair(1, "call_1", "srv:fs/read", "a"),
        assistant_tool_pair(2, "call_2", "srv:fs/read", "b"),
    ]);

    let two_pairs_same_message = build_history(vec![Message {
        id: MessageId(1),
        role: Role::Assistant,
        blocks: vec![
            ContentBlock::ToolUse {
                id: ToolCallId::new("call_1"),
                name: Arc::from("srv:fs/read"),
                input: Arc::new(serde_json::json!({})),
            },
            ContentBlock::ToolUse {
                id: ToolCallId::new("call_2"),
                name: Arc::from("srv:fs/read"),
                input: Arc::new(serde_json::json!({})),
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_1"),
                content: Arc::from("a"),
                is_error: false,
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new("call_2"),
                content: Arc::from("b"),
                is_error: false,
            },
        ],
    }]);

    let mut clear_one = SendPlan::new();
    clear_one.clear_tool_results([ToolCallId::new("call_1")]);

    let mut clear_all = SendPlan::new();
    clear_all.clear_tool_results([ToolCallId::new("call_1"), ToolCallId::new("call_2")]);

    let mut clear_nonexistent = SendPlan::new();
    clear_nonexistent.clear_tool_results([ToolCallId::new("call_ghost")]);

    let mut clear_one_in_same_message = SendPlan::new();
    clear_one_in_same_message.clear_tool_results([ToolCallId::new("call_2")]);

    let scenarios: Vec<(Vector<Message>, SendPlan)> = vec![
        (two_pairs_separate_messages.clone(), clear_one),
        (two_pairs_separate_messages.clone(), clear_all),
        (two_pairs_separate_messages.clone(), clear_nonexistent),
        (two_pairs_separate_messages.clone(), SendPlan::new()),
        (two_pairs_same_message.clone(), clear_one_in_same_message),
    ];

    for (history, plan) in scenarios {
        let projected = project(&history, &plan, None);
        assert_eq!(
            tool_use_ids(&projected),
            tool_result_ids(&projected),
            "ToolUse 与 ToolResult 的 id 集合必须恒等，不能出现落单的一半"
        );
    }
}
