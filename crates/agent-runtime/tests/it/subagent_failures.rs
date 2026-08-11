//! 029 验收的三条「不顺」路径：子 agent 失败、spawn 撞上限、轮内取消。
//!
//! 共同的判据是 003 的哲学跨 agent 版：**部分失败不中止**。一个子 agent 挂了
//! 不该把父那一轮拖垮，父收到一条 `is_error` 的 tool_result 照常接着干——
//! 模型比我们更知道这个失败要不要紧。

use crate::support;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use agent_core::{AgentId, AgentLimits, ContentBlock, Failure, Session, TurnStatus};
use agent_runtime::{ToolTable, run_turn};

use crate::support::routed::{Route, RoutedServer};

const USAGE_STOP: &str = r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#;

fn text_reply(needle: &'static str, content: &str) -> Route {
    Route::sse(
        needle,
        vec![
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{content}"}},"finish_reason":null}}]}}"#
            ),
            USAGE_STOP.to_string(),
            "data: [DONE]".to_string(),
        ],
    )
}

/// 一跳里发 `count` 个 spawn，task 文本是 `子任务编号 N`。
fn spawn_batch(needle: &'static str, count: usize) -> Route {
    let mut lines: Vec<String> = (0..count)
        .map(|i| {
            format!(
                r#"data: {{"choices":[{{"index":0,"delta":{{"tool_calls":[{{"index":{i},"id":"call_{i}","type":"function","function":{{"name":"srv_3Aagent_2Fspawn","arguments":"{{\"task\": \"子任务编号 {i}\"}}"}}}}]}}}}]}}"#
            )
        })
        .collect();
    lines.push(
        r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#
            .to_string(),
    );
    lines.push("data: [DONE]".to_string());
    Route::sse(needle, lines)
}

fn ctx_and_session(port: u16, dir: &std::path::Path) -> (agent_runtime::RunnerCtx, Session) {
    let (ctx, _events) = support::build_ctx_agent_aware(
        port,
        dir,
        ToolTable::builtin().with_spawn(AgentLimits::default()),
    );
    (ctx, Session::new(AgentId::root()))
}

fn tool_results(session: &Session) -> Vec<(String, bool)> {
    session
        .messages()
        .iter()
        .flat_map(|m| m.blocks.clone())
        .filter_map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => Some((content.to_string(), is_error)),
            _ => None,
        })
        .collect()
}

/// 一个子 agent 的 provider 回 402（余额耗尽，`ErrorClass::Exhausted`——不可重试）
/// → 那个子 agent 落 `Failed`，父收到 `is_error` 的 tool_result，另一个子 agent
/// 的结果照常，这一轮照常走到 `Done`。
#[test]
fn one_child_failing_becomes_an_is_error_tool_result_and_the_parent_carries_on() {
    let dir = support::temp_dir("subagent-402");
    let server = RoutedServer::start(vec![
        text_reply("结果A", "汇总：只拿到甲。"),
        text_reply("任务A", "结果A：甲是 1。"),
        Route::http_error(
            "任务B",
            402,
            r#"{"error":{"message":"Insufficient Balance","type":"unknown_error"}}"#,
        ),
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务A：查甲\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务B：查乙\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
                "data: [DONE]",
            ],
        ),
    ]);
    let (mut ctx, mut session) = ctx_and_session(server.port, &dir);

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "分头去查甲和乙"));
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "一个子 agent 挂了不该拖垮这一轮"
    );

    let results = tool_results(&session);
    assert_eq!(results.len(), 2, "{results:#?}");
    assert_eq!(results[0], ("结果A：甲是 1。".to_string(), false));
    assert!(
        results[1].1,
        "挂掉的那个子 agent 该是 is_error：{results:#?}"
    );
    assert!(
        results[1].0.contains("Exhausted"),
        "错误摘要该说清是哪一类：{}",
        results[1].0
    );

    assert_eq!(
        session.status_of(&AgentId::new("root/a2")),
        TurnStatus::Failed(Failure::Provider(agent_core::ErrorClass::Exhausted))
    );
}

/// 一跳里要 spawn 九个：前八个成立，**第九个撞 `max_children` 被拒**，模型收到
/// 一条 `is_error` 的 tool_result（决策 20：让它自己收敛），loop 照常走完。
#[test]
fn the_ninth_child_is_refused_as_an_is_error_tool_result_and_the_loop_keeps_going() {
    let dir = support::temp_dir("subagent-limit");
    let server = RoutedServer::start(vec![
        text_reply("小结", "汇总：八份都收到了。"),
        text_reply("子任务编号", "小结：干完了。"),
        spawn_batch("", 9),
    ]);
    let (mut ctx, mut session) = ctx_and_session(server.port, &dir);

    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "把活拆成九份"));
    assert_eq!(
        status,
        TurnStatus::Done { truncated: false },
        "撞上限不该让 loop 断掉"
    );

    let results = tool_results(&session);
    assert_eq!(results.len(), 9, "九个槽位全都收敛了：{results:#?}");
    assert_eq!(
        results.iter().filter(|(_, is_error)| *is_error).count(),
        1,
        "只有第九个该失败"
    );
    let refused = &results[8];
    assert!(refused.1);
    assert!(
        refused.0.contains('8') && refused.0.contains("活着的直接子 agent"),
        "拒绝文案要带上当时的数字，模型才知道该收敛到几：{}",
        refused.0
    );
    assert_eq!(session.agent_limits().max_children, 8);
    assert!(session.is_live(&AgentId::new("root/a8")));
    assert!(
        !session.is_live(&AgentId::new("root/a9")),
        "第九个压根没被建出来"
    );
}

/// 轮内 Ctrl-C：两个子 agent 的在飞流被取消标志斩断，会话落 `Failed(Cancelled)`。
///
/// 这一条盯的是 029 新出现的形态——**取消发生时 root 自己没有任何 IO 在飞**
/// （它在 `ToolsPending` 上等子 agent）。M2 那条「流上回来一个 Cancelled」的路
/// 对 root 不存在，泵必须替它说这一声，见 `runner::speak_for_root_on_cancel`。
#[test]
fn cancelling_mid_turn_cuts_every_child_and_fails_the_session() {
    let dir = support::temp_dir("subagent-cancel");
    let server = RoutedServer::start(vec![
        text_reply("任务A", "结果A：甲是 1。").after(Duration::from_secs(5)),
        text_reply("任务B", "结果B：乙是 2。").after(Duration::from_secs(5)),
        Route::sse(
            "",
            vec![
                r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务A：查甲\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务B：查乙\"}"}}]}}]}"#,
                r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
                "data: [DONE]",
            ],
        ),
    ]);
    let (ctx, mut session) = ctx_and_session(server.port, &dir);
    // 超时预算远大于这条测试的时间尺度：观察到的终态必须是取消标志起的作用，
    // 不是我们自己的超时机制抢跑撞上同一个终态。
    let mut ctx = ctx.with_provider_timeout(Duration::from_secs(30));

    let cancel = ctx.cancel_flag();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel.store(true, Ordering::Relaxed);
    });

    let start = Instant::now();
    let status = agent_runtime::block_on(run_turn(&mut session, &mut ctx, "分头去查甲和乙"));
    let elapsed = start.elapsed();

    assert_eq!(status, TurnStatus::Failed(Failure::Cancelled));
    assert!(session.tool_slots().is_empty(), "016：取消要把槽位全弃");
    assert!(
        elapsed < Duration::from_secs(3),
        "该在置位之后的几个 poll 间隔内收尾，不该等到子 agent 那两个 5s 的响应，实际 {elapsed:?}"
    );
    for child in ["root/a1", "root/a2"] {
        assert_eq!(
            session.status_of(&AgentId::new(child)),
            TurnStatus::Failed(Failure::Cancelled),
            "{child} 的在飞流也该被同一个标志斩断"
        );
    }
}
