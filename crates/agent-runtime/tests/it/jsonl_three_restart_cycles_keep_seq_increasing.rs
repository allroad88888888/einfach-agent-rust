//! 回归测试（独测 agent 抓到的真 bug 1，2026-08-02）：连续「起会话 → 写一轮 →
//! drop 后端 → 重开」周期会永久搞坏会话文件。
//!
//! ## 根因
//!
//! `RunnerCtx::persisted_seq`（`persist::sync` 用来判断「这个 seq 有没有告诉过
//! store」的水位）在 `RunnerCtx::new` 里恒为 `None`。`persist::recover` 把整段
//! 历史从 `SessionStore` 读回来重建进 `Session`，但没有人告诉新构造的
//! `RunnerCtx` 这些 entries **已经在盘上**——下一次 `persist::sync` 会把
//! `session.history().entries()` 里那些本来就在盘上的旧条目全部当成「从未
//! 同步过」，重新 `append` 一遍：4 行的单轮文件长到 12 行，`seq` 在文件中段
//! 跌回 0，下一次启动 `History::from_parts` 撞 `SeqNotIncreasing` 硬失败，
//! 会话彻底搁浅。
//!
//! ## 修法（两处，同一族根因，缺一都会在三周期内红）
//!
//! 1. `agent_runtime::persist::seed_after_recover(&mut ctx, &session)`：恢复
//!    成功之后调一次，把 `persisted_seq` 对齐到 `session.history().entries()`
//!    里已有的最大 seq——对全新会话是无害的空操作，`agent-cli/src/main.rs` 在
//!    `RunnerCtx::new` 之后无条件调用它（不需要区分"这次是不是恢复出来的"）。
//!    管的是 **seq 通道**：没有它，entries 会被重新 append，seq 在文件中段
//!    跌回 0。
//! 2. `agent-runtime/src/jsonl/io_thread.rs` 的 `run()` 起步用
//!    `load::seed_from_disk` 把内部那份连续存活的 `SessionLog`（`mirror`）追平
//!    到文件已有内容，不再恒等于 `SessionLog::new()`。管的是 **cursor 通道**：
//!    没有它，`mirror.held` 只反映"这个进程自己见过多少条"，`SetCursor` 落盘
//!    的 `relative_cursor()` 被系统性算小；下一次重启 recover 出
//!    `cursor < entries.len()`（明明什么都没 undo 过），它自己的下一次写入被
//!    `History` 当成"覆盖 redo 尾"，上一个周期真实写过的整轮对话被一条
//!    `drop_after` 悄悄冲掉——不 panic、不报错，是比 seq 撞硬失败更隐蔽的静默
//!    数据丢失，只有连续三个周期 + 真的重放一遍才会现形（两个周期的重启测试
//!    `jsonl_restart_continues.rs` 不会碰到，它在"重启一次just写一轮"就结束了，
//!    没有第三次读盘去验证第二轮的数据有没有被冲掉）。
//!
//! ## 这个测试验证什么
//!
//! 三个周期之后：(1) 第三次 `recover` 正常成功，不撞 `SeqNotIncreasing`（钉住
//! 修法 1）；(2) 文件里全部 "entry" 记录的 `seq` 严格递增、每个只出现一次
//! （钉住修法 1）；(3) 第三次恢复出的会话 `messages().len() == 6`——三轮对话
//! 一条都不少（钉住修法 2：少了它，(1)(2) 都能过，但这里会变成 2，因为周期二的
//! 整轮数据被前面说的那条 `drop_after` 冲掉了）。**注释掉任一处修法这个测试都会
//! 红**——本文件是先红后绿的回归测试，交付时附带过两次分别关掉其中一处修法时的
//! 失败输出。

use crate::support;
use std::path::Path;

use agent_core::{AgentId, Session, TurnStatus};
use agent_runtime::RunnerCtx;

use crate::support::ScriptedResponse;

fn plain_turn(text: &'static str) -> ScriptedResponse {
    ScriptedResponse::Sse(vec![
        Box::leak(format!(r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#).into_boxed_str()),
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":5,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
        "data: [DONE]",
    ])
}

/// 从文件里把每一条 `"kind":"entry"` 记录的 `seq` 按出现顺序抠出来——不碰
/// `agent_runtime::jsonl` 的私有 `Record` 类型（那是 crate 内部实现），只靠
/// `Jsonl` 落盘格式本身是外部可观察的这条约定（`docs/issues/011-session-store.md`
/// 「Jsonl 格式」一节）。
fn entry_seqs_in_file(path: &Path) -> Vec<u64> {
    let content = std::fs::read_to_string(path).expect("read session file");
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v.get("kind").and_then(|k| k.as_str()) == Some("entry"))
        .filter_map(|v| v.get("seq").and_then(|s| s.as_u64()))
        .collect()
}

fn build_ctx(port: u16, root: &Path, backend: Box<agent_runtime::SessionBackend>) -> RunnerCtx {
    use agent_core::SessionConfig;
    use agent_providers::deepseek::DeepSeek;
    use agent_runtime::ToolTable;
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
        backend,
        Box::new(|_ev| {}),
    )
}

#[test]
fn three_consecutive_restart_cycles_keep_the_session_file_healthy() {
    let dir = support::temp_dir("three-restarts");
    let session_path = dir.join("session.jsonl");

    // ---- 周期 1：全新会话，写一轮，drop 掉 ctx（连同它的 Jsonl）。----
    {
        let port = support::spawn_scripted_server(vec![plain_turn("cycle one reply")]);
        let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
            panic!("周期 1 不该有加载错误：{e}")
        });
        let mut ctx = build_ctx(port, &dir, backend);
        let mut session = Session::new(AgentId::root());
        let status =
            agent_runtime::block_on(agent_runtime::run_turn(&mut session, &mut ctx, "cycle one"));
        assert_eq!(status, TurnStatus::Done { truncated: false });
        // 块结束，ctx（连同它的 Jsonl）drop——`Drop` 排干队列，写入真的落盘。
    }

    // ---- 周期 2、3：各自「重启」——recover 载回、seed_after_recover 对齐水位、
    // 写一轮、drop。----
    for (turn_text, reply_text) in [
        ("cycle two", "cycle two reply"),
        ("cycle three", "cycle three reply"),
    ] {
        let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
            panic!("恢复不该有加载错误：{e}")
        });
        let mut session = agent_runtime::recover(
            backend.as_ref(),
            AgentId::root(),
            agent_core::DEFAULT_HISTORY_CAP,
            &mut |k| panic!("不该有不认识的键：{k:?}"),
        )
        .unwrap_or_else(|e| {
            panic!(
                "恢复失败——这正是 bug 1 的症状（seq 在文件中段跌回 0 之后撞 SeqNotIncreasing）：{e}"
            )
        })
        .expect("前面写过东西，该恢复出 Some");

        let port = support::spawn_scripted_server(vec![plain_turn(reply_text)]);
        let mut ctx = build_ctx(port, &dir, backend);

        // 本次修复的那一步：不调这一行，这个测试会红（`persisted_seq` 停在
        // `None`，下面的 `run_turn` 触发的 `persist::sync` 会把 `session`
        // 里恢复回来的旧条目当新条目重新 append 一遍）。
        agent_runtime::persist::seed_after_recover(&mut ctx, &session);

        session.begin_turn();
        agent_runtime::persist::sync(&mut ctx, &mut session);
        let status =
            agent_runtime::block_on(agent_runtime::run_turn(&mut session, &mut ctx, turn_text));
        assert_eq!(status, TurnStatus::Done { truncated: false });
    }

    // ---- 校验：文件里的 entry 记录 seq 全程严格递增、互不重复。----
    let seqs = entry_seqs_in_file(&session_path);
    assert!(seqs.len() >= 3, "三轮下来至少该有 3 条 entry：{seqs:?}");
    assert!(
        seqs.windows(2).all(|w| w[0] < w[1]),
        "seq 必须全程严格递增（旧条目被重新 append 会在这里跌回去）：{seqs:?}"
    );
    let mut uniq = seqs.clone();
    uniq.sort_unstable();
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        seqs.len(),
        "每条 entry 应该恰好出现一次，不该被重复 append：{seqs:?}"
    );

    // ---- 第三次「重启」也必须照常工作，不是只看文件形状——真的重放一遍。----
    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("第三次恢复不该有加载错误：{e}")
    });
    let recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("第三周期该能正常恢复，不该撞 SeqNotIncreasing 搁浅")
    .expect("写过东西该恢复出 Some");
    assert_eq!(recovered.status(), TurnStatus::Done { truncated: false });
    assert_eq!(
        recovered.messages().len(),
        6,
        "三轮各一问一答，共 6 条——不多不少，没有任何一轮被静默冲掉"
    );
}
