//! 029 验收第一条：**模型真的分解任务**。
//!
//! root 首跳一次回两个 `srv:agent/spawn`（不同 task）→ 两个子 agent 各自跑自己的
//! provider 调用 → 父第二跳汇总 → `Done`。断言四件事：
//!
//! 1. 两个子 agent 的 provider 调用**时间上重叠**（并行的证据，不是「跑得快」）；
//! 2. **B 先回但父等到 A 齐了才继续**（脚本让 A 慢 B 快）；
//! 3. 消息树完整：root 四条、每个子 agent 两条，子的结论逐字进了父的 tool_result；
//! 4. `turn_id` 全树一致（决策 5：子 agent 的 entry 继承 root 那一轮的号）。

mod support;

use std::time::Duration;

use agent_core::{AgentId, AgentLimits, ContentBlock, Role, Session, TurnStatus};
use agent_runtime::{RunnerEvent, ToolTable, run_turn};

use support::routed::{Route, RoutedServer};

/// root 首跳：两个 spawn，wire 上的函数名是转义过的（`srv:agent/spawn` →
/// `srv_3Aagent_2Fspawn`，规则见 `agent-providers/src/wire/names.rs`）。
fn root_spawns_two() -> Route {
    Route::sse(
        "",
        vec![
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":null,"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务A：查甲\"}"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"srv_3Aagent_2Fspawn","arguments":"{\"task\": \"任务B：查乙\"}"}}]}}]}"#,
            r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":100,"completion_tokens":20,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":100}}"#,
            "data: [DONE]",
        ],
    )
}

fn text_reply(needle: &'static str, line: &'static str) -> Route {
    Route::sse(
        needle,
        vec![
            line,
            r#"data: {"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":50,"completion_tokens":10,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":50}}"#,
            "data: [DONE]",
        ],
    )
}

fn routes() -> Vec<Route> {
    vec![
        // 越具体越靠前：root 第二跳的请求体里既有「结果A」（子 A 的结论已经作为
        // tool_result 进了历史）也有「任务A」（它自己那条 ToolUse 的入参）。
        text_reply(
            "结果A",
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"汇总：甲是 1，乙是 2。"},"finish_reason":null}]}"#,
        ),
        text_reply(
            "任务A",
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"结果A：甲是 1。"},"finish_reason":null}]}"#,
        )
        // 子 A 慢、子 B 快：B 先回来，但父那个槽没齐，必须等 A。
        .after(Duration::from_millis(400)),
        text_reply(
            "任务B",
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"结果B：乙是 2。"},"finish_reason":null}]}"#,
        ),
        root_spawns_two(),
    ]
}

#[test]
fn two_children_run_in_parallel_and_the_parent_waits_for_both() {
    let dir = support::temp_dir("subagent-parallel");
    let server = RoutedServer::start(routes());
    let (mut ctx, events) = support::build_ctx_agent_aware(
        server.port,
        &dir,
        ToolTable::builtin().with_spawn(AgentLimits::default()),
    );
    let mut session = Session::new(AgentId::root());

    let status = run_turn(&mut session, &mut ctx, "分头去查甲和乙");
    assert_eq!(status, TurnStatus::Done { truncated: false });

    // —— 1. 并行：两个子 agent 的请求在服务端的服务区间有交叠 ——————
    assert!(
        server.overlapped("任务A", "任务B"),
        "两个子 agent 的 provider 调用该在时间上重叠：{:?}",
        server.calls().iter().map(|c| c.needle).collect::<Vec<_>>()
    );

    // —— 2. B 先回，父仍然等到 A 齐了才发第二跳 ——————————————
    let a = server.call("任务A").expect("子 A 该被服务过");
    let b = server.call("任务B").expect("子 B 该被服务过");
    let parent_hop2 = server.call("结果A").expect("父该发第二跳");
    assert!(
        b.end < a.end,
        "脚本让 B 快 A 慢，B 该先回：{:?} vs {:?}",
        b.end,
        a.end
    );
    assert!(
        parent_hop2.start > a.end,
        "父的第二跳必须在慢的那个子 agent 回来之后——不然它没在等"
    );

    // —— 3. 消息树 ————————————————————————————————
    let root = session.messages();
    assert_eq!(
        root.len(),
        4,
        "user → assistant(2×ToolUse) → assistant(2×ToolResult) → assistant(文本)：{root:#?}"
    );
    assert_eq!(root[1].blocks.len(), 2, "一跳两个 spawn：{:#?}", root[1]);
    let results: Vec<(&str, bool)> = root[2]
        .blocks
        .iter()
        .map(|b| match b {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => (&**content, *is_error),
            other => panic!("期望 ToolResult，拿到 {other:?}"),
        })
        .collect();
    assert_eq!(
        results,
        vec![("结果A：甲是 1。", false), ("结果B：乙是 2。", false)]
    );

    let child_a = AgentId::new("root/a1");
    let child_b = AgentId::new("root/a2");
    let a_msgs = session.messages_of(&child_a);
    assert_eq!(
        a_msgs.len(),
        2,
        "子 agent：task 那条 user + 它自己的回复：{a_msgs:#?}"
    );
    assert_eq!(a_msgs[0].role, Role::User);
    assert!(
        matches!(&a_msgs[0].blocks[0], ContentBlock::Text(t) if &**t == "任务A：查甲"),
        "子 agent 的第一条 user 消息就是 task 文本：{:#?}",
        a_msgs[0]
    );
    assert_eq!(session.messages_of(&child_b).len(), 2);
    assert!(session.is_live(&child_a) && session.is_live(&child_b));

    // —— 4. turn_id 全树一致 ————————————————————————
    let turns: Vec<u64> = session
        .history()
        .entries()
        .map(|e| e.meta.turn_id)
        .collect();
    assert!(!turns.is_empty());
    assert!(
        turns.iter().all(|t| *t == session.turn_id()),
        "整棵树同一个 turn_id：{turns:?}"
    );
    let agents: Vec<String> = session
        .history()
        .entries()
        .flat_map(|e| e.changes.iter().map(|c| c.key.agent().as_str().to_string()))
        .collect();
    assert!(
        agents.iter().any(|a| a == "root/a1"),
        "子 agent 的写入落在同一条日志上：{agents:?}"
    );

    // —— 事件归属：子 agent 的事件带着自己的 id ————————————————
    let events = events.borrow();
    assert!(
        events
            .iter()
            .any(|e| e.agent == child_a && matches!(e.event, RunnerEvent::TurnGuard { .. })),
        "子 A 的 GuardReport 该归属到它自己"
    );
    assert!(
        events.iter().any(|e| e.agent == AgentId::root()
            && matches!(&e.event, RunnerEvent::ToolExecuting { request, .. } if &*request.tool == "srv:agent/spawn")),
        "spawn 这次调用归属到父"
    );
}

/// `/undo` 一轮 → spawn 和两个子 agent 干的活全部回滚；再问一次，下一轮的
/// prompt 里**一个字**子树内容都没有（假服务器请求体断言，跟 027 的 `/undo`
/// 验收同一种证明方式：真回滚的判据是模型的记忆里没有它，不是我们的 UI 不显示）。
#[test]
fn undoing_the_turn_takes_the_whole_subtree_and_the_next_prompt_has_no_trace_of_it() {
    let dir = support::temp_dir("subagent-undo");
    let mut routes = routes();
    // 重问那一轮：单独一条路由，排在兜底之前。
    routes.insert(
        3,
        text_reply(
            "再问一次",
            r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"这次我自己答。"},"finish_reason":null}]}"#,
        ),
    );
    let server = RoutedServer::start(routes);
    let (mut ctx, _events) = support::build_ctx_agent_aware(
        server.port,
        &dir,
        ToolTable::builtin().with_spawn(AgentLimits::default()),
    );
    let mut session = Session::new(AgentId::root());
    assert_eq!(
        run_turn(&mut session, &mut ctx, "分头去查甲和乙"),
        TurnStatus::Done { truncated: false }
    );

    let child_a = AgentId::new("root/a1");
    assert!(!session.messages_of(&child_a).is_empty());

    let report = session.undo_turn();
    assert!(
        matches!(report, agent_core::UndoReport::Applied { .. }),
        "spawn 是 Reversible，不该被屏障挡住：{report:?}"
    );
    assert!(session.messages().is_empty(), "root 的消息全退了");
    assert!(
        session.messages_of(&child_a).is_empty(),
        "子 agent 的消息也全退了"
    );
    assert!(
        !session.is_live(&child_a),
        "spawn 被撤销之后子 agent 不在活名单上（028 的裁决）"
    );

    assert_eq!(
        run_turn(&mut session, &mut ctx, "再问一次"),
        TurnStatus::Done { truncated: false }
    );
    let reask = server.call("再问一次").expect("重问那一跳该被服务过");
    for trace in [
        "任务A",
        "任务B",
        "结果A",
        "结果B",
        "srv_3Aagent_2Fspawn\",\"arguments",
    ] {
        assert!(
            !reask.body.contains(trace),
            "撤销之后的 prompt 不该再有子树痕迹「{trace}」：{}",
            reask.body
        );
    }
}
