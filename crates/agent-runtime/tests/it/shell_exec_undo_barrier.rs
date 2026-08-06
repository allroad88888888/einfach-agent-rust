//! 027 验收第二条：shell 轮 → `/undo` 撞屏障停下 → `/undo!` 才越过。
//!
//! `srv:shell/exec` 真的执行（`ToolExecutor` 分发到 `agent_tools::shell`），
//! 派发时 `runner::run_effect` 按 `ToolTable::with_shell()` 的
//! `reversibility_of` 判定 `Irreversible` 并调 `Session::mark_irreversible`
//! ——工具结果落地的那条 entry 因此带 `barrier: true`，`Session::undo_turn`
//! 走到它要停下（`UndoReport::Blocked`），`undo_turn_force` 才越过。
//!
//! 这条测试不经 `support::build_ctx`（那个 helper 固定用 `ToolTable::builtin()`，
//! 没有 shell），手工装一份 `RunnerCtx` 换成 `ToolTable::with_shell()`。

use crate::support;
use std::sync::Arc;

use agent_core::{AgentId, Session, SessionConfig, TurnStatus, UndoReport};
use agent_providers::deepseek::DeepSeek;
use agent_runtime::{RunnerCtx, ToolTable};
use agent_tools::ToolExecutor;
use agent_transport::Client;

use crate::support::ScriptedResponse;

fn hop1_shell_call(marker: &str) -> ScriptedResponse {
    // `srv:shell/exec` 的 wire 转义：`:` → `_3A`，`/` → `_2F`
    // （`agent_providers::wire::names` 的可读档），跟其余测试文件里
    // `srv:fs/read` → `srv_3Afs_2Fread` 同一条规则。
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

fn build_ctx_with_shell(port: u16, root: &std::path::Path) -> RunnerCtx {
    let client = Client::with_config(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(50),
        agent_transport::Backoff {
            base: std::time::Duration::from_millis(10),
            max_attempts: 1,
        },
    );
    let fs = ToolExecutor::new(root).unwrap();
    RunnerCtx::new(
        Arc::new(DeepSeek),
        Arc::new(client),
        format!("http://127.0.0.1:{port}/chat/completions"),
        "fake-key".to_string(),
        fs,
        ToolTable::with_shell(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(None, |_| {}),
        Box::new(|_ev| {}),
    )
}

#[test]
fn undo_stops_at_the_shell_barrier_and_undo_force_crosses_it() {
    let dir = support::temp_dir("shell-undo-barrier");
    let marker = dir.join("ran.marker");

    let port = support::spawn_scripted_server(vec![
        hop1_shell_call(marker.to_str().unwrap()),
        hop2_end_turn(),
    ]);
    let mut ctx = build_ctx_with_shell(port, &dir);
    let mut session = Session::new(AgentId::root());

    let status = agent_runtime::run_turn(&mut session, &mut ctx, "跑个 shell 命令")
        .expect("shell execution should not be a source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert!(marker.exists(), "shell 命令真的执行过，标记文件该在");

    // `/undo`：撞上屏障，停下，游标停在屏障之后一格（`Session::undo_turn` 的
    // 文档），不静默回滚。
    let report = session.undo_turn();
    let UndoReport::Blocked { barrier_seq, .. } = report else {
        panic!("该撞上屏障停下，拿到 {report:?}");
    };

    // 屏障没被越过：那条 entry 记录的正是这次 shell 调用的结果。
    let barrier_entry = session
        .history()
        .entries()
        .find(|e| e.seq == barrier_seq)
        .unwrap();
    assert!(barrier_entry.meta.barrier);
    let describes_the_shell_call = barrier_entry.changes.iter().any(|c| {
        let (Some(prev), Some(next)) = (c.prev.as_slots(), c.next.as_slots()) else {
            return false;
        };
        prev.iter().zip(next.iter()).any(|(p, n)| {
            matches!(p.state, agent_core::SlotState::Pending)
                && matches!(n.state, agent_core::SlotState::Finished { .. })
                && &*n.tool == "srv:shell/exec"
        })
    });
    assert!(describes_the_shell_call, "{barrier_entry:#?}");

    // `/undo!`：越过它。
    let report = session.undo_turn_force();
    assert!(matches!(report, UndoReport::Applied { .. }), "{report:?}");
    assert!(session.messages().is_empty(), "越过之后这一轮该整个退掉");
}
