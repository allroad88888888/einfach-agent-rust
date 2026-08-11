//! 验收清单第一条：假 SSE 服务器走完整两跳——第一跳带 1 个 `ToolUse`，
//! runner 真执行 `srv:fs/read`（临时目录里的真文件），第二跳回 `EndTurn`。
//! 断言终态 `Done`、消息历史完整、`GuardReport` 产出。

use crate::support;
use agent_core::{AgentId, ContentBlock, Session, TurnStatus};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support::ScriptedResponse;

/// 第一跳：DeepSeek 的 wire 形状（工具名转义 `srv:fs/read` → `srv_3Afs_2Fread`，
/// 跟 `agent-providers/src/deepseek/mod.rs` 里已经验证过的录制帧同一套写法，
/// 不是这个测试自己现造的假设）。
fn hop1_tool_use() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"srv_3Afs_2Fread","arguments":"{\"path\": \"hello.txt\"}"}}]}}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
        "data: [DONE]",
    ])
}

fn hop2_end_turn() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"文件内容是 hello world"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":150,"completion_tokens":10,"prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":86}}"#,
        "data: [DONE]",
    ])
}

#[test]
fn two_hop_tool_call_then_end_turn() {
    let dir = support::temp_dir("happy-two-hop");
    std::fs::write(dir.join("hello.txt"), b"hello world").unwrap();

    let port = support::spawn_scripted_server(vec![hop1_tool_use(), hop2_end_turn()]);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "读一下 hello.txt"));

    assert_eq!(status, TurnStatus::Done { truncated: false });

    // 消息历史完整：用户 → 助手(ToolUse) → 助手(ToolResult) → 助手(文本)。
    let messages = session.messages();
    assert_eq!(messages.len(), 4, "{messages:#?}");
    assert!(matches!(messages[0].blocks[0], ContentBlock::Text(_)));
    assert!(matches!(
        messages[1].blocks[0],
        ContentBlock::ToolUse { .. }
    ));
    match &messages[2].blocks[0] {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => {
            assert!(!is_error);
            assert_eq!(&**content, "hello world");
        }
        other => panic!("期望 ToolResult，拿到 {other:?}"),
    }
    assert!(matches!(messages[3].blocks[0], ContentBlock::Text(_)));
    assert!(session.tool_slots().is_empty(), "收敛之后槽位该清空");

    // 工具真的被执行了，且可见。
    let events = events.borrow();
    assert!(
        events.iter().any(|e| matches!(e, RunnerEvent::ToolExecuting { request, .. } if &*request.tool == "srv:fs/read")),
        "该有一条 ToolExecuting：{events:#?}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunnerEvent::ToolExecuted {
                is_error: false,
                output_len: 11,
                ..
            }
        )),
        "该有一条 ToolExecuted，output_len=11（\"hello world\" 的字节数）：{events:#?}"
    );

    // 两跳各自产出一份 GuardReport（024 第一次在真实 loop 里工作）。
    let guard_reports = events
        .iter()
        .filter(|e| matches!(e, RunnerEvent::TurnGuard { .. }))
        .count();
    assert_eq!(
        guard_reports, 2,
        "两次成功的 CallProvider 各自一份 GuardReport：{events:#?}"
    );
}
