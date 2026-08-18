//! 208 验收第 6 条：崩溃恢复之后调 `self`，`turns_used` 是恢复回来的那个值。
//!
//! 落盘/重启手法照抄 `recover_limits_indep.rs` / `jsonl_restart_continues.rs`：
//! 真 `Jsonl` 落盘 → 整个 ctx（连同它的 `Jsonl`）drop 掉（`Drop` 排干队列，
//! 之前的写入这时候真的落盘）→ 全新 `Jsonl` 指向同一路径 → `agent_runtime::recover`
//! 载回，模拟「进程重启」。
//!
//! `TurnsUsed` 是一个普通槽位（`agent-core` 的 `Slot::TurnsUsed`），走跟
//! `Messages`/`Status` 一样的快照/日志重放——不像 `AgentLimits` 需要宿主在
//! `recover` 的入参里另外再说一遍（160 那条通道是它专属的，参见
//! `recover_limits_indep.rs` 的模块文档）。这条测试因此不需要任何特殊管线，
//! 钉的是「`self` 真的读了恢复出来的图，不是一份跟 `Session` 状态脱节的账」。
//!
//! # 为什么分两层断言
//!
//! 「调 self」必须经过一次新的用户轮次——`begin_turn` 会把 `TurnsUsed` 清零给
//! 新一轮用，这是 208 之外的既有规则，不是本文件发明的。所以：
//!
//! 1. `recover()` 刚回来、还没调 `begin_turn` 之前，直接读
//!    `Session::turns_used_of` 就该等于崩溃前最后一轮落盘的那个数——这是
//!    「recover 到底有没有把这个槽位带回来」的直接证据。
//! 2. 紧接着开一个新轮次真的调一次 `self`：这一轮跟崩溃前那一轮同形状（第 1 跳
//!    自读、第 2 跳收尾），跑完之后 `turns_used_of` 落回 2——如果 `begin_turn`
//!    没有真的把计数清零（比如恢复把「崩溃前的 2」错当成了继续累加的底数），
//!    这一轮跑完会落在 4 而不是 2。落在 2 证明 `self` 用的是恢复之后仍在正常
//!    继续变化的活状态，而不是卡死不动、也不是跟 `Session` 对不上的另一份账。

use agent_core::{AgentId, Session, TurnStatus};

use crate::self_indep_support::tool_result;
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

fn self_call(call_id: &'static str) -> ScriptedResponse {
    let wire = crate::self_indep_support::wire_tool_name(agent_runtime::SELF_TOOL);
    let chunk1 = serde_json::json!({
        "choices": [{
            "index": 0,
            "delta": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {"name": wire, "arguments": "{}"}
                }]
            },
            "finish_reason": null
        }]
    });
    let chunk2 = serde_json::json!({
        "choices": [{"index": 0, "delta": {"content": ""}, "finish_reason": "tool_calls"}],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 0, "prompt_cache_miss_tokens": 10}
    });
    ScriptedResponse::Sse(vec![
        Box::leak(format!("data: {chunk1}").into_boxed_str()),
        Box::leak(format!("data: {chunk2}").into_boxed_str()),
        "data: [DONE]",
    ])
}

#[test]
fn a_recovered_session_reports_the_turns_used_it_left_off_with_and_keeps_counting_from_there() {
    let dir = support::temp_dir("self-recover-turns-used");
    let session_path = dir.join("session.jsonl");

    // ---- 「进程 1」：一轮 2 跳（先 self、再收尾），跑完 turns_used 落在 2。 ----
    let port1 = support::spawn_scripted_server(vec![self_call("call_pre"), plain_turn("落盘之前的收尾")]);
    {
        let mut ctx = build_ctx_with_jsonl(port1, &dir, &session_path);
        let mut session = Session::new(AgentId::root());
        let status = agent_runtime::run_turn(&mut session, &mut ctx, "崩溃之前问一次自己")
            .expect("崩溃前那一轮不该是 source failure");
        assert_eq!(status, TurnStatus::Done { truncated: false });
        assert_eq!(session.turns_used_of(&AgentId::root()), 2);
        agent_runtime::persist::sync(&mut ctx, &mut session);
        // `ctx`（连同它的 `Jsonl`）在这里 drop——`Drop` 排干队列，写入真的落盘。
    }

    // ---- 「进程 2」：全新 `Jsonl` 指向同一路径，`recover` 载回。 ----
    let backend = agent_runtime::open_backend(Some(session_path.clone()), |e| {
        panic!("不该有加载错误：{e}")
    });
    let mut recovered = agent_runtime::recover(
        backend.as_ref(),
        AgentId::root(),
        agent_core::DEFAULT_HISTORY_CAP,
        agent_core::AgentLimits::default(),
        &mut |k| panic!("不该有不认识的键：{k:?}"),
    )
    .expect("recover 不该失败")
    .expect("崩溃前写过一轮，该恢复出 Some");

    let root = AgentId::root();

    // 第一层断言：recover 刚回来，turns_used 就是崩溃前最后一轮落盘的那个数。
    assert_eq!(
        recovered.turns_used_of(&root),
        2,
        "recover 没有把 TurnsUsed 槽位真的带回来——self 会读到一个脱节的账"
    );

    // 第二层断言：紧接着开一个新轮次真的调一次 self，它读到的是这个「继续」出来
    // 的 turns_used（新轮次第 1 跳，落在 1），不是卡死在崩溃前那个数上。
    let port2 = support::spawn_scripted_server(vec![
        self_call("call_post"),
        plain_turn("恢复之后的收尾"),
    ]);
    let mut ctx2 = build_ctx_with_jsonl(port2, &dir, &session_path);
    recovered.begin_turn();
    agent_runtime::persist::sync(&mut ctx2, &mut recovered);
    let status = agent_runtime::run_turn(&mut recovered, &mut ctx2, "恢复之后再问一次自己")
        .expect("恢复之后那一轮不该是 source failure");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    let (post_body, post_error) = tool_result(&recovered, &root, "call_post");
    assert!(!post_error, "纯读不该失败：{post_body}");
    assert_eq!(
        recovered.turns_used_of(&root),
        2,
        "恢复之后的新轮次该正常从 0 数起、跑完两跳落在 2——落在 4 就说明 begin_turn \
         没有真的清零，是拿崩溃前的 2 当底数继续累加"
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
        ToolTable::builtin().with_self(),
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
