//! Issue 100 验收第一、四条：`agent_core::value::send_plan::project` 的输出接进
//! `encode` 之后，被清掉的工具结果正文真的从请求体里消失，占位文本按清掉的条数
//! 出现，且这条组装本身仍然满足红线 11（同一份入参两次组装逐字节相同）。
//!
//! 独立测试 agent 规则：只看 099/100 两个 issue 的定死接口和公开签名，不看
//! `crates/agent-providers/src/deepseek/` 的实现体（跟 `three_providers.rs` 顶部
//! 同一条规则）。

use std::sync::Arc;

use agent_core::value::send_plan::project;
use agent_core::{CLEARED_TOOL_RESULT, ContentBlock, Message, MessageId, RequestIntent, Role,
    SendPlan, ToolCallId};
use agent_providers::{Ingredients, Provider};
use imbl::Vector;

use crate::support;

/// 三条工具往返 + 头尾各一条普通文本，工具结果正文各自不同、足够长，
/// 方便断言「消失了」而不是碰巧被别的文本包含。
fn history_with_three_tool_calls() -> Vector<Message> {
    Vector::from(vec![
        Message {
            id: MessageId(1),
            role: Role::User,
            blocks: vec![ContentBlock::Text(Arc::from("帮我查三份档案"))],
        },
        tool_msg(2, "call_alpha", "ALPHA-CONTENT-ONE-被清掉的正文"),
        tool_msg(3, "call_beta", "BETA-CONTENT-TWO-被清掉的正文"),
        tool_msg(4, "call_gamma", "GAMMA-CONTENT-THREE-被清掉的正文"),
        Message {
            id: MessageId(5),
            role: Role::Assistant,
            blocks: vec![ContentBlock::Text(Arc::from("三份都查完了"))],
        },
    ])
}

fn tool_msg(id: u64, call_id: &str, result: &str) -> Message {
    Message {
        id: MessageId(id),
        role: Role::Assistant,
        blocks: vec![
            ContentBlock::ToolUse {
                id: ToolCallId::new(call_id),
                name: Arc::from("fs/read"),
                input: Arc::new(serde_json::json!({ "path": format!("{call_id}.txt") })),
            },
            ContentBlock::ToolResult {
                id: ToolCallId::new(call_id),
                content: Arc::from(result),
                is_error: false,
            },
        ],
    }
}

fn plan_clearing_all_three() -> SendPlan {
    let mut plan = SendPlan::new();
    plan.clear_tool_results([
        ToolCallId::new("call_alpha"),
        ToolCallId::new("call_beta"),
        ToolCallId::new("call_gamma"),
    ]);
    plan
}

fn ingredients_for<'a>(
    system: &'a [agent_core::SystemChunk],
    messages: &'a [Message],
    config: &'a agent_core::SessionConfig,
) -> Ingredients<'a> {
    Ingredients {
        system,
        messages,
        tools: &[],
        late_tools: &[],
        late_system: &[],
        config,
        intent: RequestIntent::Free,
        prev_prefix: None,
    }
}

/// 验收第一条：清掉 3 条工具结果后，`encode` 出的请求体里这 3 条正文不出现，
/// 占位文本出现 3 次——不多不少（不是清了 4 次，也不是漏了一次）。
#[test]
fn clearing_three_tool_results_removes_their_content_and_placeholder_appears_three_times() {
    let history = history_with_three_tool_calls();
    let plan = plan_clearing_all_three();
    let projected = project(&history, &plan, None);

    let system = [support::sys_chunk("base", "你是查档案的助手")];
    let config = support::session_config();
    let ing = ingredients_for(&system, &projected, &config);
    let encoded = support::provider().encode(&ing);
    let body = String::from_utf8(encoded.body).expect("wire body 该是合法 utf8");

    for gone in [
        "ALPHA-CONTENT-ONE-被清掉的正文",
        "BETA-CONTENT-TWO-被清掉的正文",
        "GAMMA-CONTENT-THREE-被清掉的正文",
    ] {
        assert!(!body.contains(gone), "被清掉的正文不该出现在请求体里：{gone}\n{body}");
    }
    assert_eq!(
        body.matches(CLEARED_TOOL_RESULT).count(),
        3,
        "占位文本该出现恰好 3 次（清了 3 条）：{body}"
    );

    // ToolUse 块原样保留：三个工具名/路径参数还在，配对没破。
    for call_id in ["call_alpha", "call_beta", "call_gamma"] {
        assert!(
            body.contains(&format!("{call_id}.txt")),
            "ToolUse 的参数该原样保留：{call_id}\n{body}"
        );
    }
}

/// 验收第四条：同一份 `(历史, SendPlan)` 走「project → encode」两次，请求体
/// 逐字节相同——不是只测 `project` 本身（099 已经测过 1000 次那条），是测这条
/// 组装到 wire 字节这一步也没有引入任何不确定性。
#[test]
fn same_history_and_plan_project_then_encode_twice_is_byte_identical() {
    let history = history_with_three_tool_calls();
    let plan = plan_clearing_all_three();
    let system = [support::sys_chunk("base", "你是查档案的助手")];
    let config = support::session_config();

    let mut bodies = Vec::new();
    for _ in 0..2 {
        let projected = project(&history, &plan, None);
        let ing = ingredients_for(&system, &projected, &config);
        bodies.push(support::provider().encode(&ing).body);
    }

    assert_eq!(
        bodies[0], bodies[1],
        "同一份 (历史, SendPlan) 两次 project+encode 必须逐字节相同（红线 11）"
    );
}
