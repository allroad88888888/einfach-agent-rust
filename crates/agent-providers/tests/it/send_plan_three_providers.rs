//! Issue 100 验收第二、三条：`project` 的输出接进 `encode`，**三家（DeepSeek /
//! Kimi / GLM）都生效**，且压缩只挪动前缀镜像的 History 段——Tools / System
//! 两段不变（动了就是接错了地方）。
//!
//! 独立测试 agent 规则同 `send_plan_clearing.rs`：只看公开签名，不看三家各自
//! 的 `encode.rs` 实现体。

use std::sync::Arc;

use agent_core::value::send_plan::project;
use agent_core::{
    ContentBlock, Message, MessageId, RequestIntent, Role, Segment, SendPlan, SystemChunk,
    ToolCallId, ToolSpec,
};
use agent_providers::Ingredients;
use imbl::Vector;

use crate::support;

fn system() -> Vec<SystemChunk> {
    vec![support::sys_chunk("base", "你是查档案的助手")]
}

fn tools() -> Vec<ToolSpec> {
    vec![support::tool_spec(
        "srv:fs/read",
        "read a file",
        serde_json::json!({"type": "object"}),
    )]
}

fn history() -> Vector<Message> {
    Vector::from(vec![
        Message {
            id: MessageId(1),
            role: Role::User,
            blocks: vec![ContentBlock::Text(Arc::from("查一下这份档案"))],
        },
        Message {
            id: MessageId(2),
            role: Role::Assistant,
            blocks: vec![
                ContentBlock::ToolUse {
                    id: ToolCallId::new("call_1"),
                    name: Arc::from("fs/read"),
                    input: Arc::new(serde_json::json!({"path": "call_1.txt"})),
                },
                ContentBlock::ToolResult {
                    id: ToolCallId::new("call_1"),
                    content: Arc::from("一大段被清掉的正文内容"),
                    is_error: false,
                },
            ],
        },
        Message {
            id: MessageId(3),
            role: Role::Assistant,
            blocks: vec![ContentBlock::Text(Arc::from("查完了"))],
        },
    ])
}

fn plan_clearing_call_1() -> SendPlan {
    let mut plan = SendPlan::new();
    plan.clear_tool_results([ToolCallId::new("call_1")]);
    plan
}

/// 验收第三条：三家各自把 `project` 的输出接进 `encode`，被清的正文都不出现、
/// 占位文本都出现一次——不是只有 DeepSeek 生效。
#[test]
fn clearing_takes_effect_for_all_three_providers() {
    let history = history();
    let plan = plan_clearing_call_1();
    let projected = project(&history, &plan, None);
    let sys = system();
    let tls = tools();

    for (family, provider) in support::providers() {
        let config = support::config_for_family(family);
        let ing = Ingredients {
            system: &sys,
            messages: &projected,
            tools: &tls,
            late_tools: &[],
            late_system: &[],
            config: &config,
            intent: RequestIntent::Free,
            prev_prefix: None,
        };
        let body = String::from_utf8(provider.encode(&ing).body).unwrap();

        assert!(
            !body.contains("一大段被清掉的正文内容"),
            "{family}：被清掉的正文不该出现在请求体里：{body}"
        );
        assert_eq!(
            body.matches(agent_core::CLEARED_TOOL_RESULT).count(),
            1,
            "{family}：占位文本该出现恰好一次：{body}"
        );
    }
}

/// 验收第二条：压缩只动 History 段。同一份 system/tools，projected 前后各
/// `encode` 一次（都是冷启动，`prev_prefix: None`——这里比的是两次镜像本身，
/// 不是漂移判定），Tools / System 段的镜像逐值相等，History 段不相等。
#[test]
fn projection_only_moves_the_history_segment_of_the_prefix_mirror() {
    let full = history();
    let plan = plan_clearing_call_1();
    let projected = project(&full, &plan, None);
    let unprojected: Vec<Message> = full.iter().cloned().collect();
    let sys = system();
    let tls = tools();

    for (family, provider) in support::providers() {
        let config = support::config_for_family(family);
        let build = |messages: &[Message]| {
            provider.encode(&Ingredients {
                system: &sys,
                messages,
                tools: &tls,
                late_tools: &[],
                late_system: &[],
                config: &config,
                intent: RequestIntent::Free,
                prev_prefix: None,
            })
        };

        let before = build(&unprojected);
        let after = build(&projected);

        let seg = |encoded: &agent_providers::Encoded, segment: Segment| {
            encoded
                .prefix
                .segments
                .iter()
                .find(|s| s.segment == segment)
                .unwrap_or_else(|| panic!("{family}：前缀镜像缺 {segment:?} 段"))
                .clone()
        };

        assert_eq!(
            seg(&before, Segment::Tools),
            seg(&after, Segment::Tools),
            "{family}：清工具结果不该动 Tools 段"
        );
        assert_eq!(
            seg(&before, Segment::System),
            seg(&after, Segment::System),
            "{family}：清工具结果不该动 System 段"
        );
        assert_ne!(
            seg(&before, Segment::History),
            seg(&after, Segment::History),
            "{family}：清工具结果该改变 History 段——一个字节都没变说明投影没接进来"
        );
    }
}
