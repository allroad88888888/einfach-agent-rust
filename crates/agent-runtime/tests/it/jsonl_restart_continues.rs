//! 027 验收第三条（在 011 的 `Memory` 之外，这里换成真文件 `Jsonl`）：Jsonl
//! 会话跑几轮 → 整个 `Jsonl` 实例 drop（`Drop` 排干队列，见 `jsonl` 模块
//! 文档）→ 新建一份指向同一路径的 `Jsonl`（模拟「进程重启」）→
//! `agent_runtime::recover` 载回 → 继续对话、undo 栈还能用。
//!
//! 真 `kill -9` 的端到端由主会话用 CLI 子进程真跑；这里钉的是
//! `agent-runtime` 这一层的恢复管道本身是对的。

mod support;

use agent_core::{AgentId, Session, TurnStatus, UndoReport};

use support::ScriptedResponse;

fn plain_turn(text: &'static str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(format!(r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#).into_boxed_str()),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
    ])
}

#[test]
fn a_session_written_through_jsonl_survives_dropping_and_reopening_the_backend() {
    let dir = support::temp_dir("jsonl-restart");
    let session_path = dir.join("session.jsonl");

    let port = support::spawn_scripted_server(vec![plain_turn("你好，我在"), plain_turn("还在的")]);

    // ---- 「进程 1」：跑两轮，drop 掉 ctx（连同它持有的 Jsonl）。----
    {
        let mut ctx = build_ctx_with_jsonl(port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());

        let status = agent_runtime::run_turn(&mut session, &mut ctx, "第一句话");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        agent_runtime::persist::maybe_snapshot(&mut ctx, &session);

        session.begin_turn();
        agent_runtime::persist::sync(&mut ctx, &mut session);
        let status = agent_runtime::run_turn(&mut session, &mut ctx, "第二句话");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        assert_eq!(session.messages().len(), 4, "两轮各一问一答");
        // `ctx`（连同它的 `Jsonl`）在这个块结束时 drop——`Jsonl::drop` 关发送端
        // 再 join IO 线程，之前所有写入这时候真的落盘了。
    }

    // ---- 「进程 2」：全新 Jsonl 指向同一路径，load 回来。----
    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let mut recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .unwrap()
    .expect("写过两轮，该恢复出 Some");

    assert_eq!(recovered.messages().len(), 4, "两轮各一问一答，共 4 条");
    assert_eq!(recovered.status(), TurnStatus::Done { truncated: false });

    // undo 栈还能用：撤一整轮，退回 2 条。
    let report = recovered.undo_turn();
    assert!(
        matches!(report, UndoReport::Applied { turn_id: 2, .. }),
        "{report:?}"
    );
    assert_eq!(recovered.messages().len(), 2);

    // 接着聊：`begin_turn` + 新一轮请求正常工作，证明恢复出来的会话是活的，
    // 不是只读快照。
    let port2 = support::spawn_scripted_server(vec![plain_turn("恢复之后还能聊")]);
    let mut ctx2 = build_ctx_with_jsonl(port2, &dir, &session_path);
    recovered.begin_turn();
    agent_runtime::persist::sync(&mut ctx2, &mut recovered);
    let status = agent_runtime::run_turn(&mut recovered, &mut ctx2, "复活之后的第一句话");
    assert_eq!(status, TurnStatus::Done { truncated: false });
    assert_eq!(
        recovered.messages().len(),
        4,
        "撤回后的 2 条 + 新一轮的 2 条"
    );
}

fn build_ctx_with_jsonl(
    port: u16,
    root: &std::path::Path,
    session_path: &std::path::Path,
) -> agent_runtime::RunnerCtx {
    use agent_core::SessionConfig;
    use agent_providers::deepseek::DeepSeek;
    use agent_runtime::{RunnerCtx, ToolTable};
    use agent_tools::ToolExecutor;
    use agent_transport::Client;
    use std::sync::Arc;

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
        ToolTable::builtin(),
        Vec::new(),
        SessionConfig {
            model: Arc::from("deepseek-v4-pro"),
            temperature: None,
            max_tokens: None,
            context_window: None,
        },
        agent_runtime::open_backend(Some(session_path.to_path_buf()), |e| {
            panic!("不该有会话文件错误：{e}")
        }),
        Box::new(|_ev| {}),
    )
}
