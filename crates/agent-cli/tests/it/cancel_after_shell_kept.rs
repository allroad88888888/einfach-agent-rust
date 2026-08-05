//! 027 已裁决的策略：取消轮如果已经执行过一个不可逆工具（比如
//! `srv:shell/exec`），自动 `undo_turn` 会撞上屏障——`undo::after_cancelled_turn`
//! 这时**保留该轮**（诚实优于整洁），不像干净取消那样整轮擦除。
//!
//! 流程：第一跳工具调用真的执行一次 shell 命令（收敛之后转移表会想再调一次
//! provider），第二跳挂住不回，Ctrl-C 打断它 → `Failed(Cancelled)`。这一轮
//! 已经有一条 `barrier: true` 的 entry（shell 调用的结果），`session.undo_turn()`
//! 走到它就停，`after_cancelled_turn` 因此走 `Blocked` 分支。

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use agent_cli::undo;
use agent_core::{AgentId, Failure, Session, TurnStatus};

use support::ScriptedResponse;

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

#[test]
fn a_cancelled_turn_that_already_ran_a_shell_command_is_kept_not_erased() {
    let dir = support::temp_dir("cancel-after-shell");
    let marker = dir.join("ran.marker");

    // 第一跳：工具调用真的跑一次 shell；第二跳（收敛之后转移表想再调一次
    // provider）挂住不回，等 Ctrl-C。
    let port = support::spawn_scripted_server(vec![
        hop1_shell_call(marker.to_str().unwrap()),
        ScriptedResponse::HangAfterHeaders,
    ]);
    let mut ctx =
        support::build_ctx_with_shell(port, &dir).with_provider_timeout(Duration::from_secs(5));

    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        cancel.store(true, Ordering::Relaxed);
    });

    let mut session = Session::new(AgentId::root());
    let status = agent_runtime::run_turn(&mut session, &mut ctx, "跑个命令然后继续说点什么");

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(marker.exists(), "shell 命令已经真的执行过");

    undo::after_cancelled_turn(&mut session, &mut ctx);

    // 保留：这一轮的用户消息还在（没有被整轮擦掉）。
    assert!(
        !session.messages().is_empty(),
        "已经执行过不可逆工具的取消轮该被保留，不是整轮擦除"
    );
    // 会话没有卡死：`/undo!` 还能接着越过它（不在这里断言到底，
    // `session_undo_redo.rs`/`shell_exec_undo_barrier.rs` 已经钉过那条机制）。
}
