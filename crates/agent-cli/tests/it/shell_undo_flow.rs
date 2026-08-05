//! 027 验收第二条，走 CLI 真正会调用的那几个函数（`agent_cli::undo::undo` /
//! `undo_force`），不是绕过它们直接摆弄 `Session`——`shell_exec_undo_barrier.rs`
//! （agent-runtime 那一层）已经钉过底层机制（`Session::undo_turn`/
//! `undo_turn_force` 撞屏障、越过屏障），这里补的是「CLI 真正调用的那层胶水
//! 也接对了」：`undo::undo` 内部会 `persist::sync`，`undo::undo_force` 会额外
//! 打一行「越过了什么」。

use crate::support;
use agent_cli::undo;
use agent_core::{AgentId, Session, TurnStatus, UndoReport};

use crate::support::ScriptedResponse;

fn hop1_shell_call(marker: &str) -> ScriptedResponse {
    let arguments = format!(r#"{{\"cmd\": \"echo hi > {marker}\"}}"#);
    let line = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":null,"tool_calls":[{{"index":0,"id":"call_shell_1","type":"function","function":{{"name":"srv_3Ashell_2Fexec","arguments":"{arguments}"}}}}]}}}}]}}"#
    );
    ScriptedResponse::Sse(vec![
        Box::leak(line.into_boxed_str()),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
        "data: [DONE]",
    ])
}

fn hop2_end_turn() -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"跑完了"},"finish_reason":null}]}"#,
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":150,"completion_tokens":10,"prompt_cache_hit_tokens":64,"prompt_cache_miss_tokens":86}}"#,
        "data: [DONE]",
    ])
}

#[test]
fn undo_then_undo_force_through_the_real_cli_functions() {
    let dir = support::temp_dir("shell-undo-flow");
    let marker = dir.join("ran.marker");

    let port = support::spawn_scripted_server(vec![
        hop1_shell_call(marker.to_str().unwrap()),
        hop2_end_turn(),
    ]);
    let mut ctx = support::build_ctx_with_shell(port, &dir);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::run_turn(&mut session, &mut ctx, "跑个 shell 命令");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(marker.exists());

    // `agent_cli::undo::undo`：CLI 的 `/undo` 就是这一行。撞屏障时只退掉
    // **比屏障新**的那一条（hop2 的收尾回复），屏障本身（记录 shell 调用结果
    // 的那条 entry）和它之前的全部留着——`UndoReport::Blocked` 的文档原话：
    // 「entries 是已经回滚掉的条数（比屏障新的那些）」。
    let messages_before = session.messages().len();
    assert_eq!(
        messages_before, 4,
        "用户提问 + 助手 ToolUse + 助手 ToolResult + 助手收尾文本"
    );
    undo::undo(&mut session, &mut ctx);
    assert_eq!(
        session.messages().len(),
        3,
        "只退掉屏障之后那一条（hop2 的收尾回复）"
    );

    // `agent_cli::undo::undo_force`：`/undo!`，真的越过。
    undo::undo_force(&mut session, &mut ctx);
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");

    // 再 `/undo`：没有可撤销的了。
    let report = session.undo_turn();
    assert_eq!(report, UndoReport::Nothing);
    assert_eq!(session.cursor(), 0);

    // `undo`/`undo_force` 内部调用的 `persist::sync` 本身的正确性（游标跟
    // store 对不对得上、redo 尾会不会被误删）已经在 agent-runtime 那一层的
    // `persist::sync` 单测里钉过（`RunnerCtx::session_store` 是 crate 内私有
    // 字段，这里拿不到直接引用去重复断言同一件事）。
}
