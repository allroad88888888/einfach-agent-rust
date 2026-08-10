//! 回归：**压缩命令落盘之后，进程重启还能恢复**。
//!
//! 100 加 `Session::replace_send_plan` 时用了新 label `"replace_send_plan"`，
//! 却没往 `command/meta.rs` 的 `KNOWN_LABELS` 里注册。那是个封闭的编译期常量集，
//! `known_label()` 认不出就返回 `None`，`persist::recover` 直接
//! `RecoverError::UnknownLabel` 硬失败——**任何用过压缩的会话，重启就打不开了**。
//!
//! 100 落地时全仓 1604 个测试全绿也没抓到，因为当时调 `replace_send_plan` 的测试
//! 全停在内存里，没有一条走完「落盘 → 重启 → 恢复」。104 落地时才发现。
//!
//! `meta.rs` 自己那个 `every_known_label_maps_back_to_itself` 永远抓不到这类问题:
//! 它遍历的是 `KNOWN_LABELS` 自己，少一项照样绿。**能抓住的只有真的走一趟这条链。**
//!
//! 所以这个文件钉的不是某条命令的业务语义，是那条链本身：
//! 压缩命令写 entry → 落盘 → 新进程 → `recover` 成功且状态还在。
//! 以后 M12 再加会落 entry 的命令，照这个形状加一条。

use agent_core::{AgentId, Session, SendPlan, ToolCallId, TurnStatus};

use crate::support;
use crate::support::ScriptedResponse;

fn plain_turn(text: &'static str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
            )
            .into_boxed_str(),
        ),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
    ])
}

/// 第 4 档（清窗口）走的 `advance_boundary`：落盘之后重启，`recover` 必须成功。
#[test]
fn a_session_that_advanced_its_boundary_still_recovers_after_a_restart() {
    let dir = support::temp_dir("jsonl-restart-after-advance-boundary");
    let session_path = dir.join("session.jsonl");
    let port = support::spawn_scripted_server(vec![plain_turn("第一句回答")]);

    {
        let mut ctx = build_ctx_with_jsonl(port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        let status = agent_runtime::run_turn(&mut session, &mut ctx, "第一句话")
            .expect("first turn should not be a source failure");
        assert_eq!(status, TurnStatus::Done { truncated: false });

        // 压缩命令：把边界推到 1。这一步会落一条 label 为 `"replace_send_plan"`
        // 的 entry——正是没注册进 `KNOWN_LABELS` 时会炸掉恢复的那条。
        session
            .advance_boundary(&AgentId::root(), 1, None)
            .expect("边界从 0 推到 1 该被接受");
        assert_eq!(session.send_plan_of(&AgentId::root()).boundary(), 1);

        agent_runtime::persist::sync(&mut ctx, &mut session);
    }

    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("recover 不该失败——失败十有八九是新 label 没进 KNOWN_LABELS")
    .expect("写过东西，该恢复出 Some");

    // 恢复出来的不只是「没报错」，压缩状态本身也得在。
    assert_eq!(
        recovered.send_plan_of(&AgentId::root()).boundary(),
        1,
        "边界要跟着恢复；丢了就是「压缩与完整历史各自独立恢复」落空了一半"
    );
}

/// 第 2 档走的 `clear_tool_results` 同理。它跟 `advance_boundary` 复用同一个
/// label，所以这条在当前实现下跟上一条同源——**但它们复用同一个 label 是实现
/// 细节，不是契约**。哪天有人给第 2 档单开一个 label，这条会独立地红。
#[test]
fn a_session_that_cleared_tool_results_still_recovers_after_a_restart() {
    let dir = support::temp_dir("jsonl-restart-after-clear-tool-results");
    let session_path = dir.join("session.jsonl");
    let port = support::spawn_scripted_server(vec![plain_turn("第一句回答")]);

    {
        let mut ctx = build_ctx_with_jsonl(port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        agent_runtime::run_turn(&mut session, &mut ctx, "第一句话")
            .expect("first turn should not be a source failure");

        // 这一轮没有工具调用，所以这个 id 会落进 `unknown`、不产生 entry。
        // 用 `replace_send_plan` 直接写一个非空计划，保证真的落一条 entry
        // ——本测试要钉的是「落盘→恢复」这条链，不是第 2 档的分桶语义
        // （那在 agent-core 的 clear_tool_results_* 里已经钉死了）。
        let outcome = session.clear_tool_results(&AgentId::root(), [ToolCallId::new("call_x")]);
        assert!(outcome.unknown.len() == 1, "这轮没有工具调用，该判 unknown");

        let mut plan = SendPlan::new();
        plan.clear_tool_results([ToolCallId::new("call_x")]);
        session.replace_send_plan(&AgentId::root(), plan);

        agent_runtime::persist::sync(&mut ctx, &mut session);
    }

    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("recover 不该失败——失败十有八九是新 label 没进 KNOWN_LABELS")
    .expect("写过东西，该恢复出 Some");

    assert_eq!(
        recovered.send_plan_of(&AgentId::root()).cleared().len(),
        1,
        "已清列表要跟着恢复"
    );
}

/// 第 3 档（摘要回写）走的 `apply_summary`。它**不复用** `"replace_send_plan"`，
/// 用的是新 label `"apply_summary"`（一条 entry 同时动两个槽位，label 要回答的是
/// 「当时发生了什么」）——所以它正是这个文件开头那段说的、必须走一趟
/// 「落盘 → 重启 → 恢复」才拦得住的那类改动。
#[test]
fn a_session_that_applied_a_summary_still_recovers_after_a_restart() {
    let dir = support::temp_dir("jsonl-restart-after-apply-summary");
    let session_path = dir.join("session.jsonl");
    let port = support::spawn_scripted_server(vec![plain_turn("第一句回答")]);

    let id = {
        let mut ctx = build_ctx_with_jsonl(port, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        agent_runtime::run_turn(&mut session, &mut ctx, "第一句话")
            .expect("first turn should not be a source failure");

        let id = session
            .apply_summary(&AgentId::root(), 1, std::sync::Arc::from("前一条的摘要"))
            .expect("边界从 0 推到 1、带一份摘要，该被接受");

        agent_runtime::persist::sync(&mut ctx, &mut session);
        id
    };

    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("recover 不该失败——失败十有八九是新 label 没进 KNOWN_LABELS")
    .expect("写过东西，该恢复出 Some");

    // 三件事一起恢复：边界、引用、正文。少任何一件，恢复出来的会话就是一份
    // 「边界推了但摘要取不到」的状态——投影会把边界作废，整段历史重新全价发。
    let plan = recovered.send_plan_of(&AgentId::root());
    assert_eq!(plan.boundary(), 1, "边界要跟着恢复");
    assert_eq!(plan.summary(), Some(&id), "摘要引用要跟着恢复");
    assert_eq!(
        recovered
            .summary_text(&AgentId::root(), &id)
            .as_deref(),
        Some("前一条的摘要"),
        "摘要正文要跟着恢复"
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
