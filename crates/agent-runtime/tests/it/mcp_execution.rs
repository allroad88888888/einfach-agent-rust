//! 043 验收第二条：模型发起 `mcp:` 调用 → dispatch 走**第四路**（不进
//! `ToolExecutor`）→ 结果作为 `tool_result` 进下一轮 prompt；MCP 失败（server 返回
//! JSON-RPC error）→ `is_error` 的 `tool_result`，loop 继续（和 spawn refuse 同精神，
//! 不 panic 不卡死）。
//!
//! 「不进 ToolExecutor」怎么证明：`ToolExecutor` 对 `mcp:everything/echo` 只会返回
//! `unknown_tool` 错误，绝造不出 `"Echo: hi"` 这段内容——所以一条 `is_error: false`
//! 且内容等于假 server 回值的 `ToolResult`，就是走了第四路的铁证。

use crate::support;
use agent_core::{AgentId, ContentBlock, Session, TurnStatus};
use agent_runtime::{run_turn, RunnerEvent};

use crate::support::mcp;

fn tool_result_block(session: &Session) -> (String, bool) {
    for message in session.messages() {
        for block in &message.blocks {
            if let ContentBlock::ToolResult {
                content, is_error, ..
            } = block
            {
                return (content.to_string(), *is_error);
            }
        }
    }
    panic!("消息历史里该有一条 ToolResult：{:#?}", session.messages());
}

#[test]
fn mcp_call_takes_the_fourth_path_and_becomes_a_tool_result() {
    let dir = support::temp_dir("mcp-exec-ok");
    let script = mcp::call_script("0", r#"{"content":[{"type":"text","text":"Echo: hi"}]}"#);
    let port = support::spawn_scripted_server(vec![
        mcp::hop_tool_use("mcp_3Aeverything_2Fecho", "call_echo"),
        mcp::hop_end_turn(),
    ]);
    let (mut ctx, events) = mcp::build_ctx(
        port,
        &dir,
        "everything",
        vec![mcp::tool_entry("everything", "echo", true)],
        &script,
    );
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "echo 一下");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // 结果来自 MCP server，不是 ToolExecutor（后者对 mcp: 名只会 unknown_tool）。
    let (content, is_error) = tool_result_block(&session);
    assert_eq!(content, "Echo: hi");
    assert!(!is_error);

    // 第四路铁证：ToolExecuting 的 request.tool 是 mcp: 全名，location 恒 Server。
    let events = events.borrow();
    assert!(
        events.iter().any(|e| matches!(
            &e.event,
            RunnerEvent::ToolExecuting { request, .. }
                if &*request.tool == "mcp:everything/echo"
                    && request.location == agent_core::Location::Server
        )),
        "该有一条指向 mcp:everything/echo 的 ToolExecuting：{events:#?}"
    );
}

#[test]
fn mcp_server_error_becomes_an_is_error_tool_result_and_the_loop_continues() {
    let dir = support::temp_dir("mcp-exec-err");
    let script = mcp::call_error_script(-32000, "boom from server");
    let port = support::spawn_scripted_server(vec![
        mcp::hop_tool_use("mcp_3Aeverything_2Fecho", "call_echo"),
        mcp::hop_end_turn(),
    ]);
    let (mut ctx, _events) = mcp::build_ctx(
        port,
        &dir,
        "everything",
        vec![mcp::tool_entry("everything", "echo", true)],
        &script,
    );
    let mut session = Session::new(AgentId::root());

    // server 报错不该 panic 也不该卡死——loop 照常走到 hop2 的 EndTurn。
    let status = run_turn(&mut session, &mut ctx, "echo 一下");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (content, is_error) = tool_result_block(&session);
    assert!(is_error, "server 返回 error 该落 is_error 的 tool_result");
    assert!(
        content.contains("boom from server"),
        "错误内容该带上 server 的 message：{content}"
    );
}
